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

fn nested_unresolved() {
    outer(
        inner(),
    );
}

fn many_unresolved() {
    call00();
    call01();
    call02();
    call03();
    call04();
    call05();
    call06();
    call07();
    call08();
    call09();
    call10();
    call11();
    call12();
    call13();
}
