use serde::{Deserialize, Serialize};
use typeweld::Api;

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub enum Role {
    Admin,
    Member { team: String },
}

fn main() {}
