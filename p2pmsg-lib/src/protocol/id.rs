#[derive(Clone,Eq, PartialEq)]
pub struct RawId([u8;32]);

#[derive(Clone,Eq, PartialEq)]
pub struct FriendlyId(String);


impl From<RawId> for FriendlyId {
    fn from(_id:RawId) -> Self {
        unimplemented!()
    }
}