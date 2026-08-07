//! Typed handles for each Lager net type.
//!
//! Handles are created from a client (e.g. [`crate::LagerBox::gpio`]) and
//! are cheap: no network traffic happens until the first method call, and
//! the box resolves/validates the net name on every request — mirroring the
//! lazy resolution of the Python `Net.get` API.

pub mod adc;
pub mod arm;
pub mod battery;
pub mod ble;
pub mod blufi;
pub mod dac;
pub mod debug;
pub mod dfu;
pub mod eload;
pub mod energy;
pub mod gpio;
pub mod i2c;
pub mod router;
pub mod scope;
#[cfg(feature = "rtt")]
pub mod rtt;
#[cfg(any(feature = "uart", feature = "rtt"))]
pub(crate) mod sio;
pub mod solar;
pub mod spi;
pub mod supply;
pub mod thermocouple;
#[cfg(feature = "uart")]
pub mod uart;
pub mod usb;
pub mod watt;
pub mod webcam;
pub mod wifi;

/// Generates the blocking and async handle structs for one net type from a
/// single method table, so the two API flavors can never drift. Each method
/// is a thin wrapper over a pure `ops::*` function returning a
/// [`crate::wire::Op`], which the client executes.
macro_rules! net_handle {
    (
        $(#[$doc:meta])*
        sync: $Sync:ident,
        async: $Async:ident,
        methods: {
            $(
                $(#[$mdoc:meta])*
                fn $method:ident ( $( $arg:ident : $ty:ty ),* $(,)? ) -> $ret:ty = $op:path;
            )*
        }
    ) => {
        $(#[$doc])*
        #[cfg(feature = "blocking")]
        #[derive(Clone)]
        pub struct $Sync<'a> {
            pub(crate) client: &'a $crate::client::LagerBox,
            pub(crate) name: String,
        }

        #[cfg(feature = "blocking")]
        impl<'a> $Sync<'a> {
            /// Name of the net this handle drives.
            pub fn name(&self) -> &str {
                &self.name
            }

            $(
                $(#[$mdoc])*
                pub fn $method(&self, $($arg: $ty),*) -> $crate::error::Result<$ret> {
                    self.client.run($op(&self.name, $($arg),*))
                }
            )*
        }

        $(#[$doc])*
        #[cfg(feature = "async")]
        #[derive(Clone)]
        pub struct $Async<'a> {
            pub(crate) client: &'a $crate::async_client::AsyncLagerBox,
            pub(crate) name: String,
        }

        #[cfg(feature = "async")]
        impl<'a> $Async<'a> {
            /// Name of the net this handle drives.
            pub fn name(&self) -> &str {
                &self.name
            }

            $(
                $(#[$mdoc])*
                pub async fn $method(&self, $($arg: $ty),*) -> $crate::error::Result<$ret> {
                    self.client.run($op(&self.name, $($arg),*)).await
                }
            )*
        }
    };
}

pub(crate) use net_handle;

/// Like [`net_handle!`] but for box-level capabilities (BLE, WiFi, BluFi)
/// that drive the box's own hardware rather than a saved net: the handle has
/// no net name, and each method's `ops::*` function takes only its own
/// arguments.
macro_rules! box_handle {
    (
        $(#[$doc:meta])*
        sync: $Sync:ident,
        async: $Async:ident,
        methods: {
            $(
                $(#[$mdoc:meta])*
                fn $method:ident ( $( $arg:ident : $ty:ty ),* $(,)? ) -> $ret:ty = $op:path;
            )*
        }
    ) => {
        $(#[$doc])*
        #[cfg(feature = "blocking")]
        #[derive(Clone)]
        pub struct $Sync<'a> {
            pub(crate) client: &'a $crate::client::LagerBox,
        }

        #[cfg(feature = "blocking")]
        impl<'a> $Sync<'a> {
            $(
                $(#[$mdoc])*
                pub fn $method(&self, $($arg: $ty),*) -> $crate::error::Result<$ret> {
                    self.client.run($op($($arg),*))
                }
            )*
        }

        $(#[$doc])*
        #[cfg(feature = "async")]
        #[derive(Clone)]
        pub struct $Async<'a> {
            pub(crate) client: &'a $crate::async_client::AsyncLagerBox,
        }

        #[cfg(feature = "async")]
        impl<'a> $Async<'a> {
            $(
                $(#[$mdoc])*
                pub async fn $method(&self, $($arg: $ty),*) -> $crate::error::Result<$ret> {
                    self.client.run($op($($arg),*)).await
                }
            )*
        }
    };
}

pub(crate) use box_handle;
