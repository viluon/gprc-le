#![recursion_limit = "1024"]
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::{sleep, Duration};
use tonic::{transport::{Channel, Server}, Request, Response, Status};
use futures::{stream, Stream, StreamExt};

use leader_election_service::leader_election_service_server::{LeaderElectionService, LeaderElectionServiceServer};
use leader_election_service::leader_election_service_client::LeaderElectionServiceClient;
use leader_election_service::{NotifyMessage, NotifyResponse, ProbeMessage, ProbeResponse};

pub mod leader_election_service {
    tonic::include_proto!("me.viluon.le");
}

const DELAY_MODIFIER: u64 = 100;

#[derive(Debug, Clone)]
pub struct Node {
    id: u64,
    left_addr: String,
    right_addr: String,
    state: Arc<Mutex<NodeState>>
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeState {
    Candidate { phase: u64, last_phase_probed: u64 },
    Defeated { leader: Option<u64> },
    Leader,
}

impl Default for NodeState {
    fn default() -> Self {
        NodeState::Candidate { phase: 1, last_phase_probed: 0 }
    }
}

impl Node {
    fn next_phase(&self, state: &mut MutexGuard<NodeState>) {
        match **state {
            NodeState::Candidate { phase, last_phase_probed } => {
                assert!(last_phase_probed == phase);
                **state = NodeState::Candidate { phase: phase + 1, last_phase_probed };
            },
            _ => panic!("next_phase() called on non-candidate node ({:?})", *state)
        }
    }

    fn defeat(&self, state: &mut MutexGuard<NodeState>) {
        match **state {
            NodeState::Candidate { .. } =>
                **state = NodeState::Defeated { leader: None },
            NodeState::Defeated { .. } => (),
            NodeState::Leader => panic!("defeat() called on the leader node ({:?})", **state),
        }
    }

    async fn defeat_with_leader(&self, leader: u64) {
        let mut state = self.state.lock().await;
        let new_state = NodeState::Defeated { leader: Some(leader) };
        match *state {
            NodeState::Candidate { .. } => *state = new_state,
            NodeState::Defeated { .. } => *state = new_state,
            NodeState::Leader => panic!("defeat_with_leader() called on the leader node ({:?})", *state),
        }
    }

    fn lead(&self, state: &mut MutexGuard<NodeState>) {
        match **state {
            NodeState::Leader => (),
            NodeState::Candidate { .. } => **state = NodeState::Leader,
            NodeState::Defeated { .. } => panic!("lead() called on a defeated node ({:?})", *state),
        }
    }
}

#[tonic::async_trait]
impl LeaderElectionService for Node {
    type NotifyElectedRawStream = Pin<Box<dyn Stream<Item = Result<NotifyResponse, Status>> + Send>>;
    type ProbeRawStream = Pin<Box<dyn Stream<Item = Result<ProbeResponse, Status>> + Send>>;

    async fn probe_raw(&self, request: Request<tonic::Streaming<ProbeMessage>>)
    -> Result<Response<Self::ProbeRawStream>, Status> {
        let mut stream = request.into_inner();

        let this = self.clone();
        let pipe: async_stream::AsyncStream<Result<ProbeResponse, Status>, _> = async_stream::try_stream!{
            println!("node {} server waiting for probes", this.id);
            while let Some(req) = stream.next().await {
                let msg = (req as Result<ProbeMessage, Status>)?;
                let addr = if msg.headed_left { &this.left_addr } else { &this.right_addr };
                let client = || LeaderElectionServiceClient::connect(addr.clone());

                if msg.sender_id != this.id {
                    // forward the message
                    println!("node {} server forwarding probe to {}", this.id, addr);
                    client().await.unwrap().probe(
                        format!("node {} server", this.id),
                        msg.sender_id,
                        msg.headed_left,
                        msg.phase,
                    );
                }

                println!("node {} server waiting for lock", this.id);

                loop {
                    let mut state: MutexGuard<NodeState> = this.state.lock().await;
                    match *state {
                        NodeState::Candidate { phase, last_phase_probed } if phase == last_phase_probed => {
                            use std::cmp::Ordering;
                            match this.id.cmp(&msg.sender_id) {
                                Ordering::Less => this.next_phase(&mut state),
                                Ordering::Equal => this.lead(&mut state),
                                Ordering::Greater => this.defeat(&mut state),
                            };
                            break
                        },
                        NodeState::Candidate { .. } => sleep(Duration::from_millis(DELAY_MODIFIER)).await,
                        _ => break,
                    };
                }
                yield ProbeResponse {};
                println!("node {} server finished processing a probe!", this.id);
            }
            println!("node {} server closing connection", this.id);
        };

        println!("node {} server establishing connection", self.id);
        Ok(Response::new(Box::pin(pipe) as Self::ProbeRawStream))
    }

    async fn notify_elected_raw(&self, request: Request<tonic::Streaming<NotifyMessage>>)
    -> Result<Response<Self::NotifyElectedRawStream>, Status> {
        let mut stream = request.into_inner();

        let this = self.clone();
        let pipe: async_stream::AsyncStream<Result<NotifyResponse, Status>, _> = async_stream::try_stream!{
            while let Some(req) = stream.next().await {
                let NotifyMessage { leader_id, headed_left } = req?;
                if this.id != leader_id {
                    println!("node {} acknowledging {}'s leadership", this.id, leader_id);
                    this.defeat_with_leader(leader_id).await;

                    // forward the message
                    let addr = if headed_left { &this.left_addr } else { &this.right_addr };
                    println!("node {} forwarding election notification to {}", this.id, addr);
                    LeaderElectionServiceClient::connect(addr.clone()).await.unwrap()
                        .notify_elected(format!("node {} server", this.id), leader_id, headed_left);
                };
                yield NotifyResponse {};
            }
        };

        Ok(Response::new(Box::pin(pipe) as Self::NotifyElectedRawStream))
    }
}

impl LeaderElectionServiceClient<Channel> {
    fn probe(mut self, id: String, sender_id: u64, headed_left: bool, phase: u64) {
        let msg = ProbeMessage { sender_id, headed_left, phase };
        tokio::spawn(async move {
            match self.probe_raw(Request::new(stream::once(async { msg })))
                .await {
                    Ok(_) => println!("{} tokio::spawned gRPC call completed", id),
                    Err(e) => println!("{} tokio::spawned gRPC call failed: {}", id, e)
                }
        });
    }

    fn notify_elected(mut self, id: String, leader_id: u64, headed_left: bool) {
        let msg = NotifyMessage { leader_id, headed_left };
        tokio::spawn(async move {
            match self.notify_elected_raw(Request::new(stream::once(async { msg })))
                .await {
                    Ok(_) => println!("{} tokio::spawned gRPC call completed", id),
                    Err(e) => println!("{} tokio::spawned gRPC call failed: {}", id, e)
                }
        });
    }
}

async fn node_client(node: Node) -> Option<()> {
    sleep(Duration::from_millis(2 * DELAY_MODIFIER)).await;
    let left = LeaderElectionServiceClient::connect(node.left_addr.clone()).await.ok()?;
    let right = LeaderElectionServiceClient::connect(node.right_addr.clone()).await.ok()?;

    loop {
        sleep(Duration::from_millis(DELAY_MODIFIER)).await;
        println!("node {} client waiting for mutex lock", node.id);
        let mut state = node.state.lock().await;
        match *state {
            NodeState::Candidate { phase, last_phase_probed } if last_phase_probed != phase => {
                let headed_left = phase % 2 == 0;
                let (target, addr) =
                    if headed_left { (&left, &node.left_addr[..]) } else { (&right, &node.right_addr[..]) };
                *state = NodeState::Candidate { phase, last_phase_probed: phase };
                println!("node {} sending probe to {} (phase {})", node.id, addr, phase);
                // FIXME is this correct?
                target.clone().probe(format!("node {} client", node.id), node.id, headed_left, phase);
                println!("node {} sent a probe", node.id);
                Some(())
            },
            NodeState::Candidate { .. } => Some(()),
            NodeState::Defeated { .. } => {
                println!("node {} is defeated", node.id);
                None
            },
            NodeState::Leader => {
                println!("node {} is the leader", node.id);
                left.clone().notify_elected(format!("node {} client", node.id), node.id, true);
                // let _ = right.clone().notify_elected(format!("node {} client", node.id), node.id, false);
                None
            },
        }?
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use futures::future;
    use std::io::stdin;

    loop {
        let mut buffer = String::new();
        stdin().read_line(&mut buffer)?;

        let node_ids = buffer
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect::<Vec<_>>();

        let first_port = 50000u16;
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

            futures.push(future::join(async move { server.await.expect("oops") }, node_client(node)));
        }

        tokio::runtime::Runtime::new()?.block_on(async {
            future::join_all(futures).await;
        });
    }
}
