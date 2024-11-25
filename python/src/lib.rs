pub mod logs;
#[cfg(feature = "receive")]
pub mod meshtastic;
#[cfg(feature = "receive")]
pub mod message_handler;
pub mod punch;
pub mod python;
pub mod status;
pub mod time;

/// This module contains structs that are generated from the protocol buffer (protobuf)
/// definitions. These structs and enums are not edited directly, but are instead generated at
/// build time.
pub mod protobufs {
    #![allow(non_snake_case)]
    include!(concat!(env!("OUT_DIR"), "/yaroc.rs"));
}
