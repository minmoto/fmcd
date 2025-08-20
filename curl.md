# Fedimint Clientd (fmcd) Curl Examples

## Quick Setup
All requests require Basic Authentication with username `fmcd` and your configured password.

```bash
# Set your password (replace with your actual password)
FMCD_PASS="bdb056904c8971cedf717265176f99e25f0c43e9f8294c69967b184c3dca768e"
FMCD_URL="http://127.0.0.1:7070"
FEDERATION_ID="15db8cb4f1ec8e484d73b889372bec94812580f929e8148b7437d359af422cd3"
GATEWAY_ID="035f2f7912e0f570841d5c0d8976a40af0dcca5609198436f596e78d2c851ee58a"
```

## Admin Endpoints

### Get Federation Info
```bash
# Get info for all connected federations
curl -s -u "fmcd:$FMCD_PASS" "$FMCD_URL/v2/admin/info" | jq .

# List federations with balances
curl -s -u "fmcd:$FMCD_PASS" "$FMCD_URL/v2/admin/info" | jq 'to_entries | map({id: .key, name: .value.meta.federation_name, totalAmountMsat: .value.totalAmountMsat})'
```

### List Operations
```bash
# Get recent operations for a federation
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/admin/operations" \
  -H "Content-Type: application/json" \
  -d "{
    \"federationId\": \"$FEDERATION_ID\",
    \"limit\": 50
  }" | jq
```

### Join Federation
```bash
# Join a new federation with an invite code
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/admin/join" \
  -H "Content-Type: application/json" \
  -d '{
    "inviteCode": "fed11qgqrgvnhwden5te0v9k8q6rp9ekh2arfdeukuet595cr2ttpd3jhq6rzve6zuer9wchxvetyd938gcewvdhk6tcqqysptkuvknc7erjgf4em3zfh90kffqf9srujn6q53d6r056qd5apxw6jxgcqqqqqq"
  }' | jq
```

### List Federations
```bash
# Get list of all connected federations
curl -s -u "fmcd:$FMCD_PASS" "$FMCD_URL/v2/admin/federations" | jq
```

### Backup Federation
```bash
# Create a backup of a federation
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/admin/backup" \
  -H "Content-Type: application/json" \
  -d "{
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Restore Federation
```bash
# Restore a federation from backup
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/admin/restore" \
  -H "Content-Type: application/json" \
  -d '{
    "backup": "YOUR_BACKUP_STRING_HERE"
  }' | jq
```

### Get Version
```bash
# Get fmcd version information
curl -s -u "fmcd:$FMCD_PASS" "$FMCD_URL/v2/admin/version" | jq
```

### Get Module Info
```bash
# Get information about federation modules
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/admin/module" \
  -H "Content-Type: application/json" \
  -d "{
    \"federationId\": \"$FEDERATION_ID\",
    \"module\": \"ln\"
  }" | jq
```

### Get Config
```bash
# Get current configuration
curl -s -u "fmcd:$FMCD_PASS" "$FMCD_URL/v2/admin/config" | jq
```

## Lightning (LN) Endpoints

### List Gateways
```bash
# Get available gateways for a federation
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/ln/gateways" \
  -H "Content-Type: application/json" \
  -d "{
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Create Invoice
```bash
# Generate a Lightning invoice
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/ln/invoice" \
  -H "Content-Type: application/json" \
  -d "{
    \"amountMsat\": 1000000,
    \"description\": \"Test invoice\",
    \"expiryTime\": 3600,
    \"gatewayId\": \"$GATEWAY_ID\",
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Pay Invoice
```bash
# Pay a Lightning invoice
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/ln/pay" \
  -H "Content-Type: application/json" \
  -d "{
    \"paymentInfo\": \"lnbc100n1p3ehk5...\",
    \"gatewayId\": \"$GATEWAY_ID\",
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Check Payment Status
```bash
# Get status of a Lightning payment
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/ln/status" \
  -H "Content-Type: application/json" \
  -d "{
    \"operationId\": \"OPERATION_ID_HERE\",
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

## On-chain Endpoints

### Get Deposit Address
```bash
# Generate a Bitcoin deposit address
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/onchain/deposit-address" \
  -H "Content-Type: application/json" \
  -d "{
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Await Deposit
```bash
# Wait for a deposit to be confirmed (with timeout)
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/onchain/await-deposit" \
  -H "Content-Type: application/json" \
  -d "{
    \"operationId\": \"OPERATION_ID_HERE\",
    \"federationId\": \"$FEDERATION_ID\",
    \"timeout\": 600
  }" | jq
```

### Withdraw to Address
```bash
# Withdraw Bitcoin to an on-chain address
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/onchain/withdraw" \
  -H "Content-Type: application/json" \
  -d "{
    \"address\": \"tb1qexampleaddress...\",
    \"amountSats\": 50000,
    \"feeRateSatsPerVbyte\": 5,
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

## Mint Endpoints

### Encode Notes
```bash
# Encode ecash notes
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/mint/encode-notes" \
  -H "Content-Type: application/json" \
  -d "{
    \"notes\": \"NOTES_DATA_HERE\",
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Decode Notes
```bash
# Decode ecash notes
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/mint/decode-notes" \
  -H "Content-Type: application/json" \
  -d "{
    \"notes\": \"ENCODED_NOTES_HERE\",
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Split Notes
```bash
# Split ecash notes into smaller denominations
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/mint/split" \
  -H "Content-Type: application/json" \
  -d "{
    \"notes\": \"NOTES_TO_SPLIT\",
    \"amountMsat\": 500000,
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Combine Notes
```bash
# Combine multiple ecash notes
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/mint/combine" \
  -H "Content-Type: application/json" \
  -d "{
    \"notes\": [\"NOTE1\", \"NOTE2\"],
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Spend Notes
```bash
# Spend ecash notes
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/mint/spend" \
  -H "Content-Type: application/json" \
  -d "{
    \"amountMsat\": 100000,
    \"allowOverpay\": true,
    \"timeout\": 60,
    \"includeInvite\": false,
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Validate Notes
```bash
# Validate ecash notes
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/mint/validate" \
  -H "Content-Type: application/json" \
  -d "{
    \"notes\": \"NOTES_TO_VALIDATE\",
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Reissue Notes
```bash
# Reissue ecash notes
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/mint/reissue" \
  -H "Content-Type: application/json" \
  -d "{
    \"notes\": \"NOTES_TO_REISSUE\",
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

## WebSocket Examples

### Connect to WebSocket
```bash
# Using wscat (install with: npm install -g wscat)
# Note: WebSocket authentication uses Basic Auth header
FMCD_AUTH_TOKEN=$(echo -n "fmcd:$FMCD_PASS" | base64 -w0)
wscat -c "ws://localhost:7070/ws" \
  -H "Authorization: Basic $FMCD_AUTH_TOKEN"
```

### WebSocket Request Format
Once connected, send JSON requests in this format:
```json
{
  "method": "admin.info",
  "params": {},
  "id": 1
}
```

### WebSocket Methods
- `admin.info` - Get federation info
- `admin.operations` - List operations
- `admin.join` - Join federation
- `ln.invoice` - Create invoice
- `ln.pay` - Pay invoice
- `ln.gateways` - List gateways
- `onchain.deposit-address` - Get deposit address
- `onchain.withdraw` - Withdraw to address
- `mint.spend` - Spend ecash

## Testing Examples

### Complete Invoice Flow
```bash
# 1. Get a gateway
GATEWAY_RESPONSE=$(curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/ln/gateways" \
  -H "Content-Type: application/json" \
  -d "{\"federationId\": \"$FEDERATION_ID\"}")
GATEWAY_ID=$(echo $GATEWAY_RESPONSE | jq -r '.gateways[0].gatewayId')

# 2. Create an invoice
INVOICE_RESPONSE=$(curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/ln/invoice" \
  -H "Content-Type: application/json" \
  -d "{
    \"amountMsat\": 1000000,
    \"description\": \"Test payment\",
    \"gatewayId\": \"$GATEWAY_ID\",
    \"federationId\": \"$FEDERATION_ID\"
  }")
INVOICE=$(echo $INVOICE_RESPONSE | jq -r '.invoice')
OPERATION_ID=$(echo $INVOICE_RESPONSE | jq -r '.operationId')

echo "Invoice: $INVOICE"
echo "Operation ID: $OPERATION_ID"

# 3. Check payment status
curl -s -u "fmcd:$FMCD_PASS" -X POST "$FMCD_URL/v2/ln/status" \
  -H "Content-Type: application/json" \
  -d "{
    \"operationId\": \"$OPERATION_ID\",
    \"federationId\": \"$FEDERATION_ID\"
  }" | jq
```

### Check Balance
```bash
# Get total balance across all federations
curl -s -u "fmcd:$FMCD_PASS" "$FMCD_URL/v2/admin/info" | \
  jq '[.[] | .totalAmountMsat] | add | . / 1000 | "Total: \(.) sats"'
```

## Error Handling

Most endpoints will return errors in this format:
```json
{
  "error": "Error message",
  "code": "ERROR_CODE",
  "details": {}
}
```

Common HTTP status codes:
- `200` - Success
- `400` - Bad Request (invalid parameters)
- `401` - Unauthorized (check authentication)
- `404` - Not Found
- `500` - Internal Server Error

## Tips

1. Always use the correct `federationId` for your requests
2. Gateway IDs are required for Lightning operations
3. Amounts are typically in millisatoshis (msat) for Lightning, satoshis for on-chain
4. Use `jq` for pretty-printing and parsing JSON responses
5. Set shell variables for frequently used values to avoid repetition
6. Check operation status for async operations like payments and deposits
