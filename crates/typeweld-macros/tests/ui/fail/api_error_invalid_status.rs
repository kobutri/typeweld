use serde::{Deserialize, Serialize};
use typeweld::ApiError;

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag")]
pub enum UserError {
    #[status(200)]
    NotAnError,
}

fn main() {}
