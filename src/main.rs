use aws_config::meta::region::RegionProviderChain;
use aws_config::BehaviorVersion;

#[tokio::main]
async fn main() {    
    let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider)
        .load()
        .await;
}
