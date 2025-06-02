use super::model::ReadModel;

// Read model
pub struct User {
    id: String,
    pub email: String,
}

impl ReadModel for User {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}
