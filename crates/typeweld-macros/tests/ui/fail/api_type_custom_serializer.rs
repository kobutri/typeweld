use serde::{Deserialize, Serialize};
use typeweld::Api;

fn as_str<S: serde::Serializer>(value: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Custom {
    #[serde(serialize_with = "as_str")]
    pub value: i64,
}

fn main() {}
