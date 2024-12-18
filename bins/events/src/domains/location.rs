pub struct Location {
    address: Address,
    coordinates: Option<Coordinates>,
}

pub struct Address {
    line_1: String,
    line_2: Option<String>,
    state: String,
    pin_code: String,
    city: String,
}

pub struct Coordinates {
    latitude: f64,
    longitude: f64,
}
