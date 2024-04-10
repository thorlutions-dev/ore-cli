# choice is the first argument

choice=$1

echo "${SOLANA_JSON_REWARD}" >/opt/transaction.json
echo "${SOLANA_JSON_GAS}" >/opt/id.json

case $choice in
"mine")
    ore --rpc ${RPC} --keypair /opt/id.json --priority-fee ${FEE} mine --threads ${THREADS}
    ;;
"rewards")
    ore --rpc ${RPC} --keypair /opt/id.json rewards
    ;;
"claim")
    ore --rpc ${RPC} --keypair /opt/id.json --priority-fee ${FEE} claim
    ;;
*)
    echo "Invalid choice."
    ;;
esac
