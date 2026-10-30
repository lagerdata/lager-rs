//! Decoding for the debug service's hand-framed chunked RTT stream.
//!
//! The box debug service (`box/lager/debug/service.py`) answers
//! `/debug/rtt` with an `HTTP/1.0` status line, yet sends
//! `Transfer-Encoding: chunked` and writes the chunk framing itself.
//! Chunked transfer coding does not exist in HTTP/1.0, so ureq rightly
//! passes the body through untouched, framing and all. [`ChunkedReader`]
//! strips that framing so callers see only the RTT payload.
//!
//! The async client cannot use reqwest for the stream at all: hyper refuses
//! a `Transfer-Encoding` header under an HTTP/1.0 status line as a parse
//! error. It reads the response off a raw socket instead, and [`AsyncBody`]
//! is the same decoder for `tokio`'s `AsyncRead`, sharing the size parsing
//! and the end-of-stream rules.

use std::io;
#[cfg(feature = "blocking")]
use std::io::{BufRead, BufReader, Read};

/// Longest chunk-size line accepted, extensions included. The box sends
/// at most a few hex digits; anything this long is not chunk framing.
const MAX_SIZE_LINE: usize = 1024;

enum State {
    /// Expecting a chunk-size line.
    Size,
    /// Inside a chunk with this many payload bytes left.
    Data(u64),
    /// Expecting the CRLF that closes a chunk's payload.
    DataEnd,
    /// After the zero-size chunk: skipping trailer lines (async decoder;
    /// the blocking one skips them in one call).
    #[cfg(feature = "async")]
    Trailers,
    /// The terminating zero-size chunk was seen, or the stream closed
    /// between chunks.
    Done,
}

/// A reader that decodes an HTTP chunked body.
#[cfg(feature = "blocking")]
///
/// The stream ends at the zero-size chunk. It also ends cleanly if the
/// connection closes between chunks: a live log stream can be cut off
/// without its terminator, and nothing is lost when that happens on a
/// chunk boundary. A close inside a chunk is
/// [`io::ErrorKind::UnexpectedEof`], since payload bytes are missing.
pub(crate) struct ChunkedReader<R> {
    inner: BufReader<R>,
    state: State,
}

#[cfg(feature = "blocking")]
impl<R: Read> ChunkedReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        ChunkedReader {
            inner: BufReader::new(inner),
            state: State::Size,
        }
    }

    /// Read one CRLF- (or bare LF-) terminated line, without the
    /// terminator. `None` means the stream closed before any byte of it.
    fn read_line(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut line = Vec::new();
        let n = (&mut self.inner)
            .take(MAX_SIZE_LINE as u64 + 2)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(None);
        }
        if line.last() != Some(&b'\n') {
            return Err(if n > MAX_SIZE_LINE {
                invalid("chunk-size line too long")
            } else {
                unexpected_eof("stream closed inside a chunk-size line")
            });
        }
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Ok(Some(line))
    }

    /// Skip the trailer section that follows the zero-size chunk.
    fn skip_trailers(&mut self) -> io::Result<()> {
        while let Some(line) = self.read_line()? {
            if line.is_empty() {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(feature = "blocking")]
impl<R: Read> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            match self.state {
                State::Done => return Ok(0),
                #[cfg(feature = "async")]
                State::Trailers => {
                    self.skip_trailers()?;
                    self.state = State::Done;
                }
                State::Size => {
                    let Some(line) = self.read_line()? else {
                        self.state = State::Done;
                        return Ok(0);
                    };
                    match parse_size(&line)? {
                        0 => {
                            self.skip_trailers()?;
                            self.state = State::Done;
                        }
                        size => self.state = State::Data(size),
                    }
                }
                State::Data(left) => {
                    let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
                    let n = self.inner.read(&mut buf[..want])?;
                    if n == 0 {
                        return Err(unexpected_eof("stream closed inside a chunk"));
                    }
                    let left = left - n as u64;
                    self.state = if left == 0 {
                        State::DataEnd
                    } else {
                        State::Data(left)
                    };
                    return Ok(n);
                }
                State::DataEnd => match self.read_line()? {
                    Some(line) if line.is_empty() => self.state = State::Size,
                    Some(_) => return Err(invalid("chunk payload longer than its size")),
                    None => return Err(unexpected_eof("stream closed before a chunk's CRLF")),
                },
            }
        }
    }
}

/// Parse a chunk-size line: hex digits, optionally followed by
/// `;extension`s, which are ignored.
fn parse_size(line: &[u8]) -> io::Result<u64> {
    let digits = line.split(|&b| b == b';').next().unwrap_or_default();
    let digits = std::str::from_utf8(digits)
        .map_err(|_| invalid("chunk size is not ASCII"))?
        .trim();
    u64::from_str_radix(digits, 16).map_err(|_| invalid("chunk size is not hex"))
}

/// The response body of a raw-socket request, decoded for `AsyncRead`:
/// chunked framing removed, a `Content-Length` honored, or read to close.
#[cfg(feature = "async")]
pub(crate) struct AsyncBody<R> {
    inner: tokio::io::BufReader<R>,
    framing: Framing,
    /// A line read so far, for chunk-size lines that arrive in pieces.
    line: Vec<u8>,
}

#[cfg(feature = "async")]
enum Framing {
    Chunked(State),
    Length(u64),
    UntilClose,
}

#[cfg(feature = "async")]
impl<R: tokio::io::AsyncRead + Unpin> AsyncBody<R> {
    /// A body already positioned past the response head. `inner` keeps
    /// whatever the head read buffered.
    pub(crate) fn new(inner: tokio::io::BufReader<R>, chunked: bool, length: Option<u64>) -> Self {
        let framing = match (chunked, length) {
            (true, _) => Framing::Chunked(State::Size),
            (false, Some(n)) => Framing::Length(n),
            (false, None) => Framing::UntilClose,
        };
        AsyncBody {
            inner,
            framing,
            line: Vec::new(),
        }
    }

    /// Poll for one line, without its terminator. `None` means the stream
    /// closed before any byte of it, as in [`ChunkedReader`].
    fn poll_line(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<Option<Vec<u8>>>> {
        use std::task::Poll;
        use tokio::io::AsyncBufRead;

        loop {
            let available = match std::pin::Pin::new(&mut self.inner).poll_fill_buf(cx) {
                Poll::Ready(Ok(available)) => available,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            if available.is_empty() {
                return Poll::Ready(if self.line.is_empty() {
                    Ok(None)
                } else {
                    Err(unexpected_eof("stream closed inside a chunk-size line"))
                });
            }
            let (take, done) = match available.iter().position(|&b| b == b'\n') {
                Some(at) => (at + 1, true),
                None => (available.len(), false),
            };
            self.line.extend_from_slice(&available[..take]);
            std::pin::Pin::new(&mut self.inner).consume(take);
            if done {
                let mut line = std::mem::take(&mut self.line);
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Poll::Ready(Ok(Some(line)));
            }
            if self.line.len() > MAX_SIZE_LINE + 1 {
                return Poll::Ready(Err(invalid("chunk-size line too long")));
            }
        }
    }

    /// Copy up to `left` bytes of payload into `buf`. Returns the count, 0
    /// only at end of stream.
    fn poll_payload(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
        left: u64,
    ) -> std::task::Poll<io::Result<usize>> {
        use std::task::Poll;
        use tokio::io::AsyncBufRead;

        let available = match std::pin::Pin::new(&mut self.inner).poll_fill_buf(cx) {
            Poll::Ready(Ok(available)) => available,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };
        let n = available
            .len()
            .min(buf.remaining())
            .min(usize::try_from(left).unwrap_or(usize::MAX));
        buf.put_slice(&available[..n]);
        std::pin::Pin::new(&mut self.inner).consume(n);
        Poll::Ready(Ok(n))
    }
}

#[cfg(feature = "async")]
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for AsyncBody<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        use std::task::Poll;

        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            match this.framing {
                Framing::UntilClose => {
                    return std::pin::Pin::new(&mut this.inner).poll_read(cx, buf)
                }
                Framing::Length(0) | Framing::Chunked(State::Done) => return Poll::Ready(Ok(())),
                Framing::Length(left) => {
                    let n = match this.poll_payload(cx, buf, left) {
                        Poll::Ready(Ok(n)) => n,
                        other => return other.map_ok(|_| ()),
                    };
                    if n == 0 {
                        return Poll::Ready(Err(unexpected_eof("stream closed inside the body")));
                    }
                    this.framing = Framing::Length(left - n as u64);
                    return Poll::Ready(Ok(()));
                }
                Framing::Chunked(State::Data(left)) => {
                    let n = match this.poll_payload(cx, buf, left) {
                        Poll::Ready(Ok(n)) => n,
                        other => return other.map_ok(|_| ()),
                    };
                    if n == 0 {
                        return Poll::Ready(Err(unexpected_eof("stream closed inside a chunk")));
                    }
                    let left = left - n as u64;
                    this.framing = Framing::Chunked(if left == 0 {
                        State::DataEnd
                    } else {
                        State::Data(left)
                    });
                    return Poll::Ready(Ok(()));
                }
                Framing::Chunked(State::Size) => {
                    let line = match this.poll_line(cx) {
                        Poll::Ready(Ok(line)) => line,
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    };
                    this.framing = Framing::Chunked(match line {
                        None => State::Done,
                        Some(line) => match parse_size(&line) {
                            Ok(0) => State::Trailers,
                            Ok(size) => State::Data(size),
                            Err(e) => return Poll::Ready(Err(e)),
                        },
                    });
                }
                Framing::Chunked(State::DataEnd) => match this.poll_line(cx) {
                    Poll::Ready(Ok(Some(line))) if line.is_empty() => {
                        this.framing = Framing::Chunked(State::Size)
                    }
                    Poll::Ready(Ok(Some(_))) => {
                        return Poll::Ready(Err(invalid("chunk payload longer than its size")))
                    }
                    Poll::Ready(Ok(None)) => {
                        return Poll::Ready(Err(unexpected_eof(
                            "stream closed before a chunk's CRLF",
                        )))
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                },
                Framing::Chunked(State::Trailers) => match this.poll_line(cx) {
                    Poll::Ready(Ok(Some(line))) if !line.is_empty() => {}
                    Poll::Ready(Ok(_)) => this.framing = Framing::Chunked(State::Done),
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn unexpected_eof(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, msg)
}

#[cfg(all(test, feature = "async"))]
mod async_tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// Decode with the async body, the source handing out `step` bytes per
    /// read so framing splits across polls.
    fn decode_in_steps(body: &[u8], step: usize) -> io::Result<Vec<u8>> {
        let mut builder = tokio_test_reader(body, step);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let reader = tokio::io::BufReader::with_capacity(step.max(1), &mut builder);
            let mut out = Vec::new();
            AsyncBody::new(reader, true, None)
                .read_to_end(&mut out)
                .await?;
            Ok(out)
        })
    }

    fn tokio_test_reader(body: &[u8], step: usize) -> Trickle {
        Trickle {
            data: body.to_vec(),
            step,
        }
    }

    struct Trickle {
        data: Vec<u8>,
        step: usize,
    }

    impl tokio::io::AsyncRead for Trickle {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            let n = self.step.min(buf.remaining()).min(self.data.len());
            buf.put_slice(&self.data[..n]);
            self.data.drain(..n);
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// The blocking decoder's cases, which the async one must agree with.
    const CASES: &[(&[u8], &[u8])] = &[
        (
            b"19\r\nblink 46823 period=500ms\n\r\n4\r\nnext\r\n0\r\n\r\n",
            b"blink 46823 period=500ms\nnext",
        ),
        (b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n", b"hello world"),
        (b"8\r\n64\r\n\r\nff\r\n0\r\n\r\n", b"64\r\n\r\nff"),
        (
            b"A;name=val\r\n0123456789\r\n0\r\nX-Trailer: 1\r\n\r\n",
            b"0123456789",
        ),
        (b"3\nabc\n0\n\n", b"abc"),
        (b"3\r\nabc\r\n", b"abc"),
        (b"", b""),
        (b"3\r\nabc\r\n0\r\n\r\ngarbage", b"abc"),
    ];

    #[test]
    fn agrees_with_the_blocking_decoder_at_every_split() {
        for (body, want) in CASES {
            for step in [1, 2, 3, 7, 64] {
                assert_eq!(
                    decode_in_steps(body, step).unwrap(),
                    *want,
                    "body {:?} in steps of {step}",
                    String::from_utf8_lossy(body)
                );
            }
        }
    }

    #[test]
    fn rejects_what_the_blocking_decoder_rejects() {
        let cases: &[(&[u8], io::ErrorKind)] = &[
            (b"a\r\nabc", io::ErrorKind::UnexpectedEof),
            (b"3\r\nabc", io::ErrorKind::UnexpectedEof),
            (b"3", io::ErrorKind::UnexpectedEof),
            (b"zz\r\nabc\r\n", io::ErrorKind::InvalidData),
            (b"2\r\nabc\r\n", io::ErrorKind::InvalidData),
        ];
        for (body, kind) in cases {
            for step in [1, 64] {
                assert_eq!(decode_in_steps(body, step).unwrap_err().kind(), *kind);
            }
        }
        let long = vec![b'1'; MAX_SIZE_LINE + 10];
        assert_eq!(
            decode_in_steps(&long, 64).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn a_length_body_stops_at_its_length_and_a_short_one_is_an_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut out = Vec::new();
            AsyncBody::new(tokio::io::BufReader::new(&b"abcdef"[..]), false, Some(4))
                .read_to_end(&mut out)
                .await
                .unwrap();
            assert_eq!(out, b"abcd");
            let err = AsyncBody::new(tokio::io::BufReader::new(&b"ab"[..]), false, Some(4))
                .read_to_end(&mut Vec::new())
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        });
    }
}

#[cfg(all(test, feature = "blocking"))]
mod tests {
    use super::*;

    /// A reader that hands out at most `step` bytes per call, to exercise
    /// framing split across reads.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.step.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    fn decode(body: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        ChunkedReader::new(body).read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn strips_framing() {
        let body = b"19\r\nblink 46823 period=500ms\n\r\n4\r\nnext\r\n0\r\n\r\n";
        assert_eq!(decode(body).unwrap(), b"blink 46823 period=500ms\nnext");
    }

    #[test]
    fn decodes_byte_at_a_time() {
        let body = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let mut out = Vec::new();
        ChunkedReader::new(Trickle {
            data: body,
            step: 1,
        })
        .read_to_end(&mut out)
        .unwrap();
        assert_eq!(out, b"hello world");
    }

    #[test]
    fn small_caller_buffer_spans_a_chunk() {
        let mut r = ChunkedReader::new(&b"a\r\n0123456789\r\n0\r\n\r\n"[..]);
        let mut buf = [0u8; 4];
        let mut out = Vec::new();
        loop {
            let n = r.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        assert_eq!(out, b"0123456789");
    }

    #[test]
    fn payload_may_contain_crlf_and_hex_lookalikes() {
        // The firmware's own CRLFs and hex-looking lines are payload, not
        // framing: only the size prefix decides where a chunk ends.
        let body = b"8\r\n64\r\n\r\nff\r\n0\r\n\r\n";
        assert_eq!(decode(body).unwrap(), b"64\r\n\r\nff");
    }

    #[test]
    fn accepts_uppercase_hex_extensions_and_trailers() {
        let body = b"A;name=val\r\n0123456789\r\n0\r\nX-Trailer: 1\r\n\r\n";
        assert_eq!(decode(body).unwrap(), b"0123456789");
    }

    #[test]
    fn accepts_bare_lf() {
        assert_eq!(decode(b"3\nabc\n0\n\n").unwrap(), b"abc");
    }

    #[test]
    fn close_between_chunks_is_a_clean_end() {
        // The box's terminator is sent from a `finally`, but a killed
        // process or dropped link never sends it.
        assert_eq!(decode(b"3\r\nabc\r\n").unwrap(), b"abc");
        assert_eq!(decode(b"").unwrap(), b"");
    }

    #[test]
    fn close_inside_a_chunk_is_an_error() {
        let err = decode(b"a\r\nabc").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let err = decode(b"3\r\nabc").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let err = decode(b"3").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn rejects_bad_framing() {
        assert_eq!(
            decode(b"zz\r\nabc\r\n").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            decode(b"2\r\nabc\r\n").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let long = vec![b'1'; MAX_SIZE_LINE + 10];
        assert_eq!(
            decode(&long).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn nothing_after_the_terminator_is_read() {
        let body = b"3\r\nabc\r\n0\r\n\r\ngarbage";
        assert_eq!(decode(body).unwrap(), b"abc");
    }
}
