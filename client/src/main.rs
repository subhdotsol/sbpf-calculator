//! Interactive CLI for the calculator program.
//!
//!   cargo run                       # asks for the network, then for calculations
//!   cargo run -- devnet add 40 2    # one-shot, no prompts
//!
//! Networks:
//!   devnet   https://api.devnet.solana.com   (the program is deployed here)
//!   testnet  https://api.testnet.solana.com  (deploy there first)
//!   local    surfpool on 127.0.0.1:8899      (`surfpool start --network devnet`)

use {
    calculator_client::{
        default_payer_path, parse_calculation, program_id, read_keypair, Calculator, ClientError,
        Network, Op,
    },
    std::io::{self, BufRead, Write},
};

const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;

fn prompt(question: &str) -> String {
    print!("{question}");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).ok();
    line.trim().to_string()
}

fn choose_network() -> Network {
    println!("Where should the client run?");
    println!("  1) devnet");
    println!("  2) testnet");
    println!("  3) local (surfpool, a fork of devnet on your machine)");
    loop {
        match prompt("> ").parse() {
            Ok(network) => return network,
            Err(e) => println!("{e}"),
        }
    }
}

/// For the local network: make sure surfpool is up before continuing.
fn wait_for_surfpool(client: &Calculator) {
    loop {
        if client.rpc().get_version().is_ok() {
            println!("surfpool is running.");
            return;
        }
        println!();
        println!("surfpool is not running on {}.", Network::Local.rpc_url());
        println!("Start it in another terminal (it forks devnet, so the deployed program is available):");
        println!();
        println!("    surfpool start --network devnet");
        println!();
        prompt("Press Enter once it's running (Ctrl-C to quit)... ");
    }
}

fn run(network: Network, client: &Calculator, op: Op, a: u64, b: u64) {
    match client.calculate(op, a, b) {
        Ok(outcome) => {
            println!("  {op:?}({a}, {b}) = {}", outcome.value);
            println!("  result account: {}", outcome.result_account);
            println!("  signature:      {}", outcome.signature);
            if let Some(url) = network.explorer_tx_url(&outcome.signature) {
                println!("  explorer:       {url}");
            }
        }
        Err(e) => println!("  failed: {e}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let network = match args.first() {
        Some(n) => n.parse().unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        }),
        None => choose_network(),
    };

    let payer_path = default_payer_path();
    let payer = read_keypair(&payer_path).unwrap_or_else(|e| {
        eprintln!("{e}\nCreate a wallet with `solana-keygen new` or set KEYPAIR=/path/to/keypair.json");
        std::process::exit(1)
    });
    let client = Calculator::new(network.rpc_url(), payer, program_id());

    println!("network: {network} ({})", network.rpc_url());
    if network == Network::Local {
        wait_for_surfpool(&client);
    }

    match client.check_deployed() {
        Ok(()) => println!("program: {} (deployed)", program_id()),
        Err(ClientError::NotDeployed(id)) => {
            eprintln!("program {id} is not deployed on {network}. Deploy it first:");
            eprintln!("    solana airdrop 1 -u {network}");
            eprintln!("    sbpf deploy calculator {network}        (from practice/calculator)");
            std::process::exit(1)
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }

    match client.balance() {
        Ok(lamports) => println!(
            "payer:   {} ({:.4} SOL, from {})",
            client.payer(),
            lamports as f64 / LAMPORTS_PER_SOL,
            payer_path.display()
        ),
        Err(e) => eprintln!("could not read balance: {e}"),
    }

    // One-shot mode: `cargo run -- devnet add 40 2`
    if args.len() == 4 {
        match parse_calculation(&args[1..].join(" ")) {
            Ok((op, a, b)) => run(network, &client, op, a, b),
            Err(e) => eprintln!("{e}"),
        }
        return;
    }

    println!();
    println!("Enter a calculation like `add 40 2` (ops: add sub mul div), or `q` to quit.");
    loop {
        let input = prompt("calc> ");
        if input.is_empty() {
            continue;
        }
        if matches!(input.as_str(), "q" | "quit" | "exit") {
            break;
        }
        match parse_calculation(&input) {
            Ok((op, a, b)) => run(network, &client, op, a, b),
            Err(e) => println!("  {e}"),
        }
    }
}
