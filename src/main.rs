
use std::sync::Arc;
use std::sync::Mutex;
use tokio::time::{sleep, Duration};
use tonic::{transport::{Channel, Server}, Request, Response, Status};

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
        NodeState::Candidate { phase: 0 }
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
        use std::cmp::Ordering;
        let ProbeMessage { sender_id } = request.into_inner();
        match self.id.cmp(&sender_id) {
            Ordering::Less => self.next_phase(),
            Ordering::Equal => self.lead(),
            Ordering::Greater => self.defeat(),
        }
        Ok(Response::new(ProbeResponse {}))
    }

    async fn notify_elected(&self, request: Request<NotifyMessage>)
    -> Result<Response<NotifyResponse>, Status> {
        let NotifyMessage { leader_id } = request.into_inner();
        if self.id != leader_id {
            self.defeat_with_leader(leader_id);
        }
        Ok(Response::new(NotifyResponse {}))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use futures::future;

    let first_port = 50000u16;
    let node_ids = (0u16 .. 5).collect::<Vec<_>>();
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

        let client = async move {
            sleep(Duration::from_millis(200)).await;
            let mut left = LeaderElectionServiceClient::connect(node.left_addr).await.ok()?;
            let mut right = LeaderElectionServiceClient::connect(node.right_addr).await.ok()?;

            loop {
                match *node.state.lock().ok()? {
                    NodeState::Candidate { phase } => {
                        let target = if phase % 2 == 0 { &mut left } else { &mut right };
                        println!("node {} sending probe to {:?}", node_id, target);
                        match target.probe(ProbeMessage { sender_id: node.id }).await {
                            Ok(_) => (),
                            Err(e) => eprintln!("node {} failed to probe {:?}: {}", node_id, target, e),
                        }
                        println!("node {} ok", node_id);
                        Some(())
                    },
                    NodeState::Defeated { .. } => {
                        println!("node {} is defeated", node_id);
                        None
                    },
                    NodeState::Leader => {
                        println!("node {} is the leader", node_id);
                        let _ = left.notify_elected(NotifyMessage { leader_id: node.id }).await.ok()?;
                        let _ = right.notify_elected(NotifyMessage { leader_id: node.id }).await.ok()?;
                        Some(())
                    },
                }?;
            }

            Some(())
        };
        futures.push(future::join(server, client));
    }

    tokio::runtime::Runtime::new()?.block_on(async {
        future::join_all(futures).await;
    });

    Ok(())
}
