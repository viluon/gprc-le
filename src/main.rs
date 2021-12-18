
use std::sync::Arc;
use std::sync::Mutex;
use tokio::time::{sleep, Duration};
use tonic::{transport::Server, Request, Response, Status};

use leader_election_service::leader_election_service_server::{LeaderElectionService, LeaderElectionServiceServer};
use leader_election_service::leader_election_service_client::LeaderElectionServiceClient;
use leader_election_service::{NotifyMessage, NotifyResponse, ProbeMessage, ProbeResponse};

pub mod leader_election_service {
    tonic::include_proto!("me.viluon.le");
}

#[derive(Debug, Clone)]
pub struct Node {
    id: u64,
    left_addr: String,
    right_addr: String,
    state: Arc<Mutex<NodeState>>
}

#[derive(Debug, Clone)]
enum NodeState {
    Candidate { phase: u64 },
    Defeated { leader: Option<u64> },
    Leader,
}

impl Default for NodeState {
    fn default() -> Self {
        NodeState::Candidate { phase: 1 }
    }
}

impl Node {
    fn next_phase(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            NodeState::Candidate { phase } => {
                *state = NodeState::Candidate { phase: phase + 1 };
            },
            _ => panic!("next_phase() called on non-candidate node ({:?})", *state)
        }
    }

    fn defeat(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            NodeState::Candidate { .. } =>
                *state = NodeState::Defeated { leader: Some(self.id) },
            NodeState::Defeated { .. } => (),
            NodeState::Leader => panic!("defeat() called on the leader node ({:?})", *state),
        }
    }

    fn defeat_with_leader(&self, leader: u64) {
        let mut state = self.state.lock().unwrap();
        let new_state = NodeState::Defeated { leader: Some(leader) };
        match *state {
            NodeState::Candidate { .. } => *state = new_state,
            NodeState::Defeated { .. } => *state = new_state,
            NodeState::Leader => panic!("defeat_with_leader() called on the leader node ({:?})", *state),
        }
    }

    fn lead(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            NodeState::Leader => (),
            NodeState::Candidate { .. } => *state = NodeState::Leader,
            NodeState::Defeated { .. } => panic!("lead() called on a defeated node ({:?})", *state),
        }
    }
}

#[tonic::async_trait]
impl LeaderElectionService for Node {
    async fn probe(&self, request: Request<ProbeMessage>)
    -> Result<Response<ProbeResponse>, Status> {
        let state = self.state.lock().unwrap().clone();

        match state {
            NodeState::Candidate { .. } => {
                use std::cmp::Ordering;
                let ProbeMessage { sender_id, .. } = request.into_inner();
                match self.id.cmp(&sender_id) {
                    Ordering::Less => self.next_phase(),
                    Ordering::Equal => self.lead(),
                    Ordering::Greater => self.defeat(),
                }
            },
            _ => {
                // forward the message
                let msg = request.into_inner();
                let addr = if msg.headed_left { &self.left_addr } else { &self.right_addr };
                if let Ok(mut client) = LeaderElectionServiceClient::connect(addr.clone()).await {
                    client.probe(msg).await?;
                } else {
                    eprintln!("couldn't connect to node {}", addr);
                }
            }
        }

        Ok(Response::new(ProbeResponse {}))
    }

    async fn notify_elected(&self, request: Request<NotifyMessage>)
    -> Result<Response<NotifyResponse>, Status> {
        let NotifyMessage { leader_id, headed_left } = request.into_inner();
        if self.id != leader_id {
            self.defeat_with_leader(leader_id);

            // forward the message
            let addr = if headed_left { &self.left_addr } else { &self.right_addr };
            if let Ok(mut client) = LeaderElectionServiceClient::connect(addr.clone()).await {
                client.notify_elected(NotifyMessage { leader_id, headed_left }).await?;
            } else {
                eprintln!("couldn't connect to node {}", addr);
            }
        }
        Ok(Response::new(NotifyResponse {}))
    }
}

async fn node_client(node: Node) -> Option<()> {
    let mut last_phase = 0;

    sleep(Duration::from_millis(200)).await;
    let mut left = LeaderElectionServiceClient::connect(node.left_addr.clone()).await.ok()?;
    let mut right = LeaderElectionServiceClient::connect(node.right_addr.clone()).await.ok()?;

    loop {
        let state = node.state.lock().unwrap().clone();
        match state {
            NodeState::Candidate { phase } if last_phase != phase => {
                let headed_left = phase % 2 == 0;
                let (target, addr) =
                    if headed_left { (&mut left, &node.left_addr[..]) } else { (&mut right, &node.right_addr[..]) };

                println!("node {} sending probe to {} (phase {})", node.id, addr, phase);
                match target.probe(ProbeMessage { sender_id: node.id, headed_left }).await {
                    Ok(_) => (),
                    Err(e) => eprintln!("node {} failed to probe {}: {}", node.id, addr, e),
                }
                println!("node {} ok", node.id);
                last_phase = phase;
                Some(())
            },
            NodeState::Candidate { .. } => {
                // FIXME busy wait
                Some(())
            },
            NodeState::Defeated { .. } => {
                println!("node {} is defeated", node.id);
                None
            },
            NodeState::Leader => {
                println!("node {} is the leader", node.id);
                let _ = left.notify_elected(NotifyMessage { leader_id: node.id, headed_left: true }).await.ok()?;
                let _ = right.notify_elected(NotifyMessage { leader_id: node.id, headed_left: false }).await.ok()?;
                None
            },
        }?
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use futures::future;

    let first_port = 50000u16;
    let node_ids = (0u16 .. 3).collect::<Vec<_>>();
    let get_addr = |id: u16| format!("[::1]:{}", first_port + id);

    let mut futures = vec![];
    for (i, &node_id) in node_ids.iter().enumerate() {
        let prev_id = node_ids[(node_ids.len() + i - 1) % node_ids.len()];
        let next_id = node_ids[(i + 1) % node_ids.len()];

        println!("node {} listening on {}", node_id, get_addr(node_id));
        let node = Node {
            id: node_id.into(),
            left_addr: "http://".to_string() + &get_addr(prev_id),
            right_addr: "http://".to_string() + &get_addr(next_id),
            state: Arc::default(),
        };

        let server = Server::builder()
            .add_service(LeaderElectionServiceServer::new(node.clone()))
            .serve(get_addr(node_id).parse().unwrap());

        futures.push(future::join(server, node_client(node)));
    }

    tokio::runtime::Runtime::new()?.block_on(async {
        future::join_all(futures).await;
    });

    Ok(())
}
