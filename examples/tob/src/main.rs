use std::error::Error;
use std::time::Duration;
use tob::{Message, spawn_cluster};
use tokio::time::timeout;

const NODES: u32 = 4;
const PER_NODE: usize = 5;

fn render(message: &Message) -> String {
    format!(
        "{}/{}",
        u32::from(message.sender),
        String::from_utf8_lossy(&message.payload)
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut cluster = spawn_cluster(NODES)?;

    for tob in &cluster {
        for k in 0..PER_NODE {
            tob.broadcast(format!("m{k}"));
        }
    }

    let total = NODES as usize * PER_NODE;
    let mut logs = Vec::new();
    for tob in &mut cluster {
        let mut log = Vec::new();
        while log.len() < total {
            let message = timeout(Duration::from_secs(10), tob.deliver())
                .await?
                .ok_or("node stopped")?;
            log.push(message);
        }
        logs.push(log);
    }

    for (tob, log) in cluster.iter().zip(&logs) {
        let order: Vec<String> = log.iter().map(render).collect();
        println!("node {}: {}", u32::from(tob.id()), order.join(" "));
    }

    if logs.windows(2).all(|pair| pair[0] == pair[1]) {
        println!("all {NODES} nodes delivered {total} messages in the same order");
        Ok(())
    } else {
        Err("nodes delivered messages in different orders".into())
    }
}
