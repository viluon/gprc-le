
use leader_election_service::leader_election_service_client::LeaderElectionServiceClient;
use leader_election_service::{ProbeMessage, ProbeResponse, NotifyMessage, NotifyResponse};

pub mod leader_election_service {
    tonic::include_proto!("me.viluon.le");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = LeaderElectionServiceClient::connect("http://[::1]:50051").await?;

    let probe = tonic::Request::new(ProbeMessage {
        sender_id: 0,
    });

    let response = client.probe(probe).await?;
    println!("RESPONSE={:?}", response);

    Ok(())
}
