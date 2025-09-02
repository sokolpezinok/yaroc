use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("Common error: {0}")]
    CommonError(#[from] yaroc_common::error::Error),
    #[error("Parse error")]
    ParseError,
    #[error("Protobuf parse error: {0}")]
    ProstDecodeError(#[from] prost::DecodeError),
    #[error("Protobuf parse error: {0}")]
    FemtopbDecodeError(femtopb::error::DecodeError),
    #[error("Connection error")]
    ConnectionError,
    #[error("Channel send error")]
    ChannelSendError,
    #[error("Encryption error: cannot decrypt without a key")]
    EncryptionError {
        /// Node ID of the Meshtastic node sending this message
        node_id: u32,
        /// Meshtastic channel ID
        channel_id: u32,
    },
    #[error("Value error: wrong or missing data")]
    ValueError,
}
