pub struct Location {
    address: Address,
    coordinates: Option<Coordinates>,
}

pub struct Address {
    line_1: String,
    line_2: Option<String>,
}

pub struct Coordinates {
    latitude: f64,
    longitude: f64,
}
