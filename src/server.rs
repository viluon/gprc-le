
use std::sync::Arc;
use std::sync::Mutex;
use tonic::{transport::Server, Request, Response, Status};

use leader_election_service::leader_election_service_server::{LeaderElectionService, LeaderElectionServiceServer};
use leader_election_service::{NotifyMessage, NotifyResponse, ProbeMessage, ProbeResponse};

pub mod leader_election_service {
    tonic::include_proto!("me.viluon.le");
}

#[derive(Debug, Default)]
pub struct Node {
    id: u64,
    left: u64,
    right: u64,
    state: Arc<Mutex<NodeState>>
}

#[derive(Debug)]
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
            NodeState::Defeated { .. } => *state = new_state,
            NodeState::Candidate { .. } => *state = new_state,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse().unwrap();
    let leader_election_service = Node::default();

    println!("LeaderElectionService listening on {}", addr);
    Server::builder()
        .add_service(LeaderElectionServiceServer::new(leader_election_service))
        .serve(addr)
        .await?;

    Ok(())
}
