use bytes::BufMut;
use tokio_util::codec::{Decoder, Encoder};

use super::message::Message;
use crate::error::Error;

pub struct MsgCodec {
    next_pos: usize,
}

impl MsgCodec {
    pub fn new() -> Self {
        MsgCodec { next_pos: 0 }
    }
}

impl Encoder<Message> for MsgCodec {
    type Error = Error;

    fn encode(&mut self, item: Message, buf: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        match serde_json::to_string(&item) {
            Err(e) => Err(Box::new(e)),
            Ok(data) => {
                buf.reserve(data.len() + 1);
                buf.put(data.as_bytes());
                buf.put_u8(b'\n');
                Ok(())
            }
        }
    }
}

impl Decoder for MsgCodec {
    type Item = Message;
    type Error = Error;

    fn decode(&mut self, buf: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match buf[self.next_pos..].iter().position(|b| *b == b'\n') {
            None => {
                self.next_pos = buf.len();
                Ok(None)
            }
            Some(pos) => {
                let pos = self.next_pos+pos;
                self.next_pos = 0;
                let data = buf.split_to(pos + 1);
                Ok(Some(serde_json::from_slice(&data[..pos])
                .map_err(|e| {
                    error!("Serde error {}, data {:?}, pos {}, whole data {:?}", e, &data[..pos], pos, &data);
                    e
                })?))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json() {
        let m = Message::Hello {
            msg: "Hello world".into(),
        };

        let txt = serde_json::to_string(&m).unwrap();
        println!("message looks like: {}", txt);

        let p = Message::Ping;

        let txt = serde_json::to_string(&p).unwrap();
        println!("ping looks like: {}", txt);

        let mut codec = MsgCodec::new();
        let mut buf = bytes::BytesMut::new();
        codec.encode(m.clone(), &mut buf).unwrap();

        let res = codec.decode(&mut buf).unwrap();

        assert_eq!(0, buf.len());

        match (m, res) {
            (Message::Hello { msg: m1 }, Some(Message::Hello { msg: m2 })) => assert_eq!(m1, m2),
            _ => panic!("Not equal"),
        }
    }
}

