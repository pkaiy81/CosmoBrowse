use std::string::String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    name: String,
    value: String,
}

impl Attribute {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            value: String::new(),
        }
    }

    /// Construct an attribute from a name/value pair (e.g. from a DOM
    /// `setAttribute` call).
    pub fn from_name_value(name: &str, value: &str) -> Self {
        Self {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    pub fn set_value(&mut self, value: &str) {
        self.value = value.to_string();
    }

    pub fn add_char(&mut self, c: char, is_name: bool) {
        if is_name {
            self.name.push(c);
        } else {
            self.value.push(c);
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn value(&self) -> String {
        self.value.clone()
    }
}
