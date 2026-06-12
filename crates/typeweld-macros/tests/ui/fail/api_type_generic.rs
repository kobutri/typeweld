use serde::{Deserialize, Serialize};
use typeweld::Api;

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Wrapper<T> {
    pub value: T,
}

fn main() {}
