use std::{
    env,
    io::{stdout, Write},
    time::Duration,
};

use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    nonblocking::rpc_client::RpcClient,
    rpc_config::{RpcSendTransactionConfig, RpcSimulateTransactionConfig},
};
use solana_program::instruction::Instruction;
use solana_sdk::{
    commitment_config::{CommitmentConfig, CommitmentLevel},
    compute_budget::ComputeBudgetInstruction,
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};

use crate::Miner;

fn get_env_var_u64(key: &str, default: u64) -> u64 {
    match env::var(key) {
        Ok(val) => val.parse::<u64>().unwrap_or(default),
        Err(_e) => default,
    }
}

fn get_env_var_usize(key: &str, default: usize) -> usize {
    match env::var(key) {
        Ok(val) => val.parse::<usize>().unwrap_or(default),
        Err(_e) => default,
    }
}

impl Miner {
    pub async fn send_and_confirm(
        &self,
        ixs: &[Instruction],
        dynamic_cus: bool,
        skip_confirm: bool,
    ) -> ClientResult<Signature> {
        let rpc_retries: usize = get_env_var_usize("RPC_RETRIES", 0);
        let simulation_retries: usize = get_env_var_usize("SIMULATION_RETRIES", 4);
        let gateway_retries: usize = get_env_var_usize("GATEWAY_RETRIES", 4);
        let confirm_retries: usize = get_env_var_usize("CONFIRM_RETRIES", 4);

        let confirm_delay: u64 = get_env_var_u64("CONFIRM_DELAY", 5000);
        let gateway_delay: u64 = get_env_var_u64("GATEWAY_DELAY", 2000);

        let mut stdout = stdout();
        let signer_reward = self.signer("default");
        let signer_gas = self.signer("gas");
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::confirmed());

        // Return error if balance is zero
        let balance = client
            .get_balance_with_commitment(&signer_gas.pubkey(), CommitmentConfig::confirmed())
            .await
            .unwrap();
        if balance.value <= 0 {
            return Err(ClientError {
                request: None,
                kind: ClientErrorKind::Custom("Insufficient SOL balance".into()),
            });
        }

        // Build tx
        let (mut hash, mut slot) = client
            .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
            .await
            .unwrap();
        let mut send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(rpc_retries),
            min_context_slot: Some(slot),
        };
        let mut tx = Transaction::new_with_payer(ixs, Some(&signer_gas.pubkey()));

        // Simulate if necessary
        if dynamic_cus {
            let mut sim_attempts = 0;
            'simulate: loop {
                let sim_res = client
                    .simulate_transaction_with_config(
                        &tx,
                        RpcSimulateTransactionConfig {
                            sig_verify: false,
                            replace_recent_blockhash: true,
                            commitment: Some(CommitmentConfig::confirmed()),
                            encoding: Some(UiTransactionEncoding::Base64),
                            accounts: None,
                            min_context_slot: None,
                            inner_instructions: false,
                        },
                    )
                    .await;
                match sim_res {
                    Ok(sim_res) => {
                        if let Some(err) = sim_res.value.err {
                            println!("Simulaton error: {:?}", err);
                            sim_attempts += 1;
                            if sim_attempts.gt(&simulation_retries) {
                                return Err(ClientError {
                                    request: None,
                                    kind: ClientErrorKind::Custom("Simulation failed".into()),
                                });
                            }
                        } else if let Some(units_consumed) = sim_res.value.units_consumed {
                            println!("Dynamic CUs: {:?}", units_consumed);
                            let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(
                                units_consumed as u32 + 1000,
                            );
                            let cu_price_ix =
                                ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
                            let mut final_ixs = vec![];
                            final_ixs.extend_from_slice(&[cu_budget_ix, cu_price_ix]);
                            final_ixs.extend_from_slice(ixs);
                            tx = Transaction::new_with_payer(&final_ixs, Some(&signer_gas.pubkey()));
                            break 'simulate;
                        }
                    }
                    Err(err) => {
                        println!("Simulaton error: {:?}", err);
                        sim_attempts += 1;
                        if sim_attempts.gt(&simulation_retries) {
                            return Err(ClientError {
                                request: None,
                                kind: ClientErrorKind::Custom("Simulation failed".into()),
                            });
                        }
                    }
                }
            }
        }

        // Submit tx
        tx.sign(&[&signer_reward], hash);
        let mut sigs = vec![];
        let mut attempts = 0;
        loop {
            println!("Attempt: {:?}", attempts);
            match client.send_transaction_with_config(&tx, send_cfg).await {
                Ok(sig) => {
                    sigs.push(sig);
                    println!("{:?}", sig);

                    // Confirm tx
                    if skip_confirm {
                        return Ok(sig);
                    }
                    for _ in 0..confirm_retries {
                        std::thread::sleep(Duration::from_millis(confirm_delay));
                        match client.get_signature_statuses(&sigs).await {
                            Ok(signature_statuses) => {
                                println!("Confirms: {:?}", signature_statuses.value);
                                for signature_status in signature_statuses.value {
                                    if let Some(signature_status) = signature_status.as_ref() {
                                        if signature_status.confirmation_status.is_some() {
                                            let current_commitment = signature_status
                                                .confirmation_status
                                                .as_ref()
                                                .unwrap();
                                            match current_commitment {
                                                TransactionConfirmationStatus::Processed => {}
                                                TransactionConfirmationStatus::Confirmed
                                                | TransactionConfirmationStatus::Finalized => {
                                                    println!("Transaction landed!");
                                                    return Ok(sig);
                                                }
                                            }
                                        } else {
                                            println!("No status");
                                        }
                                    }
                                }
                            }

                            // Handle confirmation errors
                            Err(err) => {
                                println!("Error: {:?}", err);
                            }
                        }
                    }
                    println!("Transaction did not land");
                }

                // Handle submit errors
                Err(err) => {
                    println!("Error {:?}", err);
                }
            }
            stdout.flush().ok();

            // Retry
            std::thread::sleep(Duration::from_millis(gateway_delay));
            (hash, slot) = client
                .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
                .await
                .unwrap();
            send_cfg = RpcSendTransactionConfig {
                skip_preflight: true,
                preflight_commitment: Some(CommitmentLevel::Confirmed),
                encoding: Some(UiTransactionEncoding::Base64),
                max_retries: Some(rpc_retries),
                min_context_slot: Some(slot),
            };
            tx.sign(&[&signer_reward], hash);
            attempts += 1;
            if attempts > gateway_retries {
                return Err(ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom("Max retries".into()),
                });
            }
        }
    }
}
