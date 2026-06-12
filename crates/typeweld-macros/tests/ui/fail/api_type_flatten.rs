use serde::{Deserialize, Serialize};
use typeweld::Api;

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Inner {
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Outer {
    #[serde(flatten)]
    pub inner: Inner,
}

fn main() {}
