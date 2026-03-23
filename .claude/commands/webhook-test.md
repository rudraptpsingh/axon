Test alert webhook delivery end-to-end.

Run: `bash scripts/e2e-webhook-config-file.sh`

Report:
- Were webhook POSTs received by the test receiver?
- How many alerts fired and what types?
- Alert severities seen (warn / critical / resolved)
- Did axon serve exit cleanly?
- Any delivery failures, timeouts, or dropped webhooks?
