fn greet(name: &str) -> String {
    let msg = format!("hello {}", name);
    println!("{}", msg);
    msg
}

fn process() {
    let result = greet("world");
    let trimmed = result.trim();
    send(trimmed);
}

fn send(data: &str) {
    println!("sending: {}", data);
}
