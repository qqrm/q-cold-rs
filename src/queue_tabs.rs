#[derive(Args)]
struct QueueCreateArgs {
    #[arg(help = "Queue tab label")]
    label: Option<String>,
    #[command(flatten)]
    client: QueueClientArgs,
}

#[derive(Args)]
struct QueueSwitchArgs {
    #[arg(help = "Queue tab id")]
    tab_id: String,
    #[command(flatten)]
    client: QueueClientArgs,
}

#[derive(Args)]
struct QueueDeleteArgs {
    #[arg(help = "Queue tab id")]
    tab_id: String,
    #[command(flatten)]
    client: QueueClientArgs,
}

#[derive(Serialize)]
struct QueueTabCreateRequest {
    label: Option<String>,
}

#[derive(Serialize)]
struct QueueTabRequest {
    tab_id: String,
}

fn create_queue(args: QueueCreateArgs) -> Result<u8> {
    let request = QueueTabCreateRequest { label: args.label };
    let response =
        QueueHttpClient::from_args(&args.client).post_json("/api/queue/tab/create", &request)?;
    print_queue_api_response(&response);
    Ok(0)
}

fn switch_queue(args: QueueSwitchArgs) -> Result<u8> {
    let request = QueueTabRequest {
        tab_id: args.tab_id,
    };
    let response =
        QueueHttpClient::from_args(&args.client).post_json("/api/queue/tab/switch", &request)?;
    print_queue_api_response(&response);
    Ok(0)
}

fn delete_queue(args: QueueDeleteArgs) -> Result<u8> {
    let request = QueueTabRequest {
        tab_id: args.tab_id,
    };
    let response =
        QueueHttpClient::from_args(&args.client).post_json("/api/queue/tab/delete", &request)?;
    print_queue_api_response(&response);
    Ok(0)
}
