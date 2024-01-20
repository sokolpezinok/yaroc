use yaroc_rs::async_serial::AsyncSerial;

#[tokio::main]
async fn main() {
    let mut serial = AsyncSerial::new("/dev/ttyUSB2").unwrap();
    let b = serial
        .call(
            "AT+CPSI?",
            r"\+CPSI: (?<serv>.*),(?<stat>.*)",
            &["serv", "stat"],
            1.0,
        )
        .await;
    println!("{:?}", b);
}
