# Model Providers

OpenForge routes by declared model capabilities rather than hard-coding one vendor.

## Provider kinds

### `openai-compatible`

Uses `POST <base_url>/chat/completions`. API keys are optional so fully local endpoints can operate without credentials. This adapter covers local engines and compatible gateways.

### `anthropic`

Uses the Anthropic Messages API. Set `api_key_env` to the name of the environment variable containing the API credential.

### `gemini`

Uses Google Gemini `generateContent`. Set `api_key_env` to the credential environment variable.

### `bedrock-aws-cli`

Invokes Bedrock Converse through the installed AWS CLI and the caller's existing AWS credential chain. OpenForge does not copy AWS credentials into execution sandboxes.

## Model metadata

Each configured model declares:

- context window;
- tool/vision/structured-output support;
- input/output prices;
- latency, quality and privacy scores;
- maximum data classification.

The router applies hard constraints first and only then scores eligible models.

## Data classification

Classification order is `PUBLIC < INTERNAL < CONFIDENTIAL < RESTRICTED`. A request cannot route to a model whose configured maximum classification is below the request's classification.

Prices are configuration data because vendor prices change. Operators must keep configured prices current if cost enforcement is expected to reflect billed provider cost.
