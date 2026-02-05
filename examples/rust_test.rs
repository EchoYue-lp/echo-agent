fn main() {
    let mut messages = Vec::new();
    messages.push("1");
    messages.push("2");
    messages.push("3");
    messages.push("4");
    messages.push("5");

    let msg2 = messages[..messages.len()].to_vec();
    let msg3 = messages[..messages.len() - 1].to_vec();

    println!("msg1: {:?}", messages);
    println!("msg2: {:?}", msg2);
    println!("msg3: {:?}", msg3);

    let v: Vec<&str> = "Mary had a little lambda".splitn(6, ' ').collect();

    println!("{:?}", v);
}
