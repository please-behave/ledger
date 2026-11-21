fn greeting() -> &'static str {
    "Hello, World!"
}

fn main() {
    println!("{}", greeting());
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greets_the_world() {
        assert_eq!(greeting(), "Hello, World!");
    }
}
