use serde::{Deserialize, Serialize};
use typeweld::Api;

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(untagged)]
pub enum Value {
    Text { text: String },
    Number { number: f64 },
}

fn main() {}
