use serde::{Deserialize, Serialize};
use typeweld::Api;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NoDerive {
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Holder {
    pub inner: NoDerive,
}

fn main() {}
