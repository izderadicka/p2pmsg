#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Message {
    Hello { msg: String },
    Ping,
    Pong,
    Terminate
}