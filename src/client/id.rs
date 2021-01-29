use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize, Serialize)]
pub struct ClientId(u64);

impl ClientId {
    pub fn random() -> Self {
        Self(rand::random::<u64>())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize, Serialize)]
pub struct MessageId(u64);

impl MessageId {
    pub fn random() -> Self {
        Self(rand::random())
    }
}
