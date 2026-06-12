use serde::{Deserialize, Serialize};
use typeweld::{api, Api, Json, Sse};

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Event {
    pub id: i32,
}

#[api(get, "/events")]
pub fn events() -> Sse<Event> {
    unimplemented!()
}

fn main() {}
