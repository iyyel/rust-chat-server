use futures::{future, pin_mut, StreamExt};

use std::collections::HashSet;

use async_std::io;
use serde::{Deserialize, Serialize};

use async_std::prelude::*;
use async_std::task;
use async_tungstenite::async_std::connect_async;
use async_tungstenite::tungstenite::protocol::Message as TungMessage;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Message<'a> {
    src_name: &'a str,
    src_addr: &'a str,
    msg_type: MessageType<'a>,
    text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
enum MessageType<'a> {
    NewPeer(&'a str), // Broadcast this message to all peers when a new peer has connected. The parameter is the name of the new peer that has connected.
    DisconPeer(&'a str), // Broadcast this message to all peers when a peer has disconnected. The parameter is the name of the peer that has disconnected.
    PeerNameAssign(&'a str), // The server sends this message to a peer when it has first connected, giving it a random name. The name is the parameter.
    PeerInfoRequest, // A peer sends this message to the server if the peer wants to retrieve peer info (PeerDataReply message is sent back to the peer).
    PeerInfoReply(PeerInfo), // If the server has received a PeerDataRequest message, a peer is asking to retrieve data about all connected peers. This resides in the PeerInfo struct.
    Private(&'a str), // A private message to the given peer. The parameter is the name of the peer receiving the message.
    Text,             // Standard broadcasted text message to all peers.
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PeerInfo {
    peers_online: i32,           // How many peers are currently online?
    peer_spots_left: i32,        // How many available spots are left for connections?
    peer_names: HashSet<String>, // What are the names of the connected peers? excluding the requesting peers name.
}

pub struct Client {
    addr: String,
    name: String,
}

impl Client {
    pub fn new(addr: String) -> Self {
        Self {
            addr,
            name: String::new(),
        }
    }

    pub async fn connect(&mut self) {
        let (sender, receiver) = futures::channel::mpsc::unbounded::<TungMessage>();

        let (ws_stream, _) = connect_async(format!("ws://{}/socket", &self.addr))
            .await
            .expect("Failed to connect");

        println!("WebSocket handshake has been successfully completed.");

        let local_addr = ws_stream.get_ref().local_addr().unwrap().to_string();

        let (write, mut read) = ws_stream.split();

        let stdin_to_ws = receiver.map(Ok).forward(write);

        // Wait until name message has been received.
        loop {
            if let Some(msg) = read.next().await {
                let msg = msg.unwrap().to_string();
                let msg: Message = serde_json::from_str(&msg).unwrap();
                let msg_type = msg.msg_type.clone();

                match msg_type {
                    MessageType::PeerNameAssign(new_name) => {
                        async_std::io::stdout()
                            .write_all(
                                format!("\n[Chat] Welcome to Rust-Chat, {}!", new_name).as_bytes(),
                            )
                            .await
                            .unwrap();
                        self.name = new_name.to_string();
                        async_std::io::stdout().flush().await.unwrap();
                        break;
                    }
                    _ => continue,
                }
            };
        }

        task::spawn(read_stdin(sender, local_addr, self.name.clone()));

        let ws_to_stdout = async {
            while let Some(msg) = read.next().await {
                let msg = msg.unwrap().to_string();
                let msg: Message = serde_json::from_str(&msg).unwrap();
                let msg_type = msg.msg_type.clone();

                match msg_type {
                    MessageType::NewPeer(peer_name) => async_std::io::stdout()
                        .write_all(
                            format!("\n[Chat] {}: {} has connected.", &msg.src_name, peer_name)
                                .as_bytes(),
                        )
                        .await
                        .unwrap(),
                    MessageType::DisconPeer(peer_name) => async_std::io::stdout()
                        .write_all(
                            format!(
                                "\n[Chat] {}: {} has disconnected.",
                                &msg.src_name, peer_name
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap(),
                    MessageType::Text => async_std::io::stdout()
                        .write_all(format!("\n[Chat] {}: {}", &msg.src_name, &msg.text).as_bytes())
                        .await
                        .unwrap(),
                    MessageType::PeerInfoRequest => async_std::io::stdout()
                        .write_all(
                            format!("\n[PeerDataRequest] {}: {}", &msg.src_name, &msg.text)
                                .as_bytes(),
                        )
                        .await
                        .unwrap(),
                    MessageType::PeerInfoReply(peer_data) => async_std::io::stdout()
                        .write_all(
                            format!("\n[PeerDataReply] {}: {:?}", &msg.src_name, peer_data)
                                .as_bytes(),
                        )
                        .await
                        .unwrap(),
                    MessageType::PeerNameAssign(name) => {
                        async_std::io::stdout()
                            .write_all(
                                format!("\n[PeerName] {}: {}, {}", &msg.src_name, &msg.text, name)
                                    .as_bytes(),
                            )
                            .await
                            .unwrap();
                    }
                    MessageType::Private(name) => async_std::io::stdout()
                        .write_all(
                            format!("\n[PM] {}: {}: {}", &msg.src_name, &msg.text, name).as_bytes(),
                        )
                        .await
                        .unwrap(),
                }
                async_std::io::stdout().flush().await.unwrap();
            }
        };

        pin_mut!(stdin_to_ws, ws_to_stdout);
        future::select(stdin_to_ws, ws_to_stdout).await;
    }
}

// Our helper method which will read data from stdin and send it along the
// sender provided.
async fn read_stdin(
    sender: futures::channel::mpsc::UnboundedSender<TungMessage>,
    local_addr: String,
    peer_name: String,
) {
    let mut stdin = io::stdin();

    loop {
        async_std::io::stdout()
            .write_all(format!("\n[Chat] {}: ", peer_name).as_bytes())
            .await
            .unwrap();
        async_std::io::stdout().flush().await.unwrap();

        let mut buf = vec![0; 1024];
        let n = match stdin.read(&mut buf).await {
            Err(_) | Ok(0) => break,
            Ok(n) => n,
        };
        buf.truncate(n);

        let mut msg = String::from_utf8(buf).unwrap();

        if msg.ends_with('\n') {
            msg.pop();
            if msg.ends_with('\r') {
                msg.pop();
            }
        }

        if msg.starts_with("pm: ") {
            let split: Vec<&str> = msg.split(" ").collect();
            let (recv_name, msg) = (split[1].to_string(), split[2].to_string());

            let msg_struct = Message {
                src_addr: local_addr.as_str(),
                src_name: peer_name.as_str(),
                msg_type: MessageType::Private(recv_name.as_str()),
                text: msg,
            };

            sender
                .unbounded_send(TungMessage::Text(
                    serde_json::to_string(&msg_struct).unwrap(),
                ))
                .unwrap();
        } else if msg.starts_with("peerdatarequest") {
            let msg_struct = Message {
                src_addr: local_addr.as_str(),
                src_name: peer_name.as_str(),
                msg_type: MessageType::PeerInfoRequest,
                text: String::from(""),
            };

            sender
                .unbounded_send(TungMessage::Text(
                    serde_json::to_string(&msg_struct).unwrap(),
                ))
                .unwrap();
        } else {
            let msg_struct = Message {
                src_addr: local_addr.as_str(),
                src_name: peer_name.as_str(),
                msg_type: MessageType::Text,
                text: msg,
            };

            sender
                .unbounded_send(TungMessage::Text(
                    serde_json::to_string(&msg_struct).unwrap(),
                ))
                .unwrap();
        }
    }
}
