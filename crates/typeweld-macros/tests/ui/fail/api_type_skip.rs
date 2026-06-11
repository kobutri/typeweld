use serde::{Deserialize, Serialize};
use typeweld::Api;

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Secretive {
    pub public: String,
    #[serde(skip)]
    pub private: String,
}

fn main() {}
