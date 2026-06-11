use serde::{Deserialize, Serialize};
use typeweld::Api;

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Bad {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: String,
}

fn main() {}
