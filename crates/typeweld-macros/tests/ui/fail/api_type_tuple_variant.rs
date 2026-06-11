use serde::{Deserialize, Serialize};
use typeweld::Api;

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(tag = "_tag")]
pub enum Event {
    Created(String),
}

fn main() {}
