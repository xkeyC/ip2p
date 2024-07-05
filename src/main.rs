use std::sync::Arc;
use tokio::net::UdpSocket;
use webrtc::stun::agent::TransactionId;
use webrtc::stun::client::ClientBuilder;
use webrtc::stun::message::{BINDING_REQUEST, Getter, Message};
use webrtc::stun::xoraddr::XorMappedAddress;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = "stun.miwifi.com:3478";
    let (handler_tx, mut handler_rx) = tokio::sync::mpsc::unbounded_channel();
    let conn = UdpSocket::bind("0.0.0.0:0").await?;
    println!("Local address: {}", conn.local_addr()?);
    println!("Connecting to: {server}");
    conn.connect(server).await?;
    let mut client = ClientBuilder::new().with_conn(Arc::new(conn)).build()?;
    let mut msg = Message::new();
    msg.build(&[Box::<TransactionId>::default(), Box::new(BINDING_REQUEST)])?;
    client.send(&msg, Some(Arc::new(handler_tx))).await?;
    if let Some(event) = handler_rx.recv().await {
        let msg = event.event_body?;
        let mut xor_addr = XorMappedAddress::default();
        xor_addr.get_from(&msg)?;
        println!("Got response: {xor_addr}");
    }
    client.close().await?;
    Ok(())
}
