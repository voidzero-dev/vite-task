# Deploy a remote cache for your GitHub repository

This guide deploys the service in your Cloudflare account and gives your repository its own cache endpoint. You can bind any **public repository on GitHub.com**. Setup reads its default branch and permits uploads from GitHub Actions `push` jobs on that branch. All cache reads are public. Private repositories and other Git providers are not supported.

**Current status:** the server and operator CLI work in this source tree. The `vp run` remote-cache client adapter is not implemented here. You can deploy and verify the HTTP service now. Automatic task uploads and reuse require a compatible client; the proposed Vite+ settings are described separately below.

## Quick start: Deploy to Cloudflare

[![Deploy to Cloudflare](https://deploy.workers.cloudflare.com/button)](https://deploy.workers.cloudflare.com/?url=https%3A%2F%2Fgithub.com%2Fvoidzero-dev%2Fvite-task%2Ftree%2Ffeat%2Fpublic-remote-cache%2Fpackages%2Fremote-cache)

The button copies this package into a new GitHub repository in your account and connects it to Workers Builds. It provisions a Worker, a D1 database, and an R2 bucket. You do not need a local checkout or GitHub Actions secrets. The repository that hosts the Worker can differ from the application repository that uses its cache. See [Cloudflare's deployment-button guide](https://developers.cloudflare.com/workers/platform/deploy-buttons/).

1. Enable R2 in your Cloudflare account and create a [Workers subdomain](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/). Workers Free is the default; R2 has separate usage allowances and billing.
2. Click the button and connect your GitHub and Cloudflare accounts. Choose a name for the new source repository, Worker, D1 database, and R2 bucket. Use dedicated storage. The storage names can differ from the Worker name.
3. Set the following variables in the setup form. They are public deployment settings, not secrets.

   | Variable           | Value                                                                  |
   | ------------------ | ---------------------------------------------------------------------- |
   | `CACHE_REPOSITORY` | Your public GitHub repository, for example `acme/web-app`              |
   | `CACHE_NAMESPACE`  | Endpoint namespace; defaults to `cache`                                |
   | `CACHE_PROFILE`    | Keep `free`; choose `paid` only when you want the larger cleanup batch |

4. Use `pnpm build` as the build command and `pnpm deploy` as the deploy command. The copied package includes its own dependency versions and lockfile. Use Node.js 22.12 or newer and the pinned pnpm version. The root directory in this new repository is `/`.
5. Check the selected build API token's permissions. Deployment needs Workers, D1, and Workers R2 Storage write access. Cloudflare manages the build token, but its [documented default permissions](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/#api-token) do not include D1. Add D1 Edit / Write to that token under **My Profile → API Tokens**, or select a token with these permissions under **Worker → Settings → Build**. Keep the token in Cloudflare; do not add it as a Worker variable or commit it.
6. Deploy. If you changed the token after the first build started, retry the build. The deploy command applies migrations, configures private storage and cleanup, binds your application repository, and checks the public endpoint. A successful log ends with `Deployment checks passed. Cache endpoint: ...`.

For a Worker named `acme-build-cache` on `my-team.workers.dev`, the default endpoint is:

```text
https://acme-build-cache.my-team.workers.dev/projects/cache
```

Use that complete URL as the client endpoint and upload-token audience. Continue with [Connect your application build](#5-connect-your-application-build) below. Opening `/` in a browser returns `404`; the service has no dashboard.

Workers Builds deploys subsequent pushes to the selected production branch. Disable non-production branch builds for this service so preview code does not share production storage. To change the deployment inputs, edit `vars.CACHE_REPOSITORY`, `vars.CACHE_NAMESPACE`, or `vars.CACHE_PROFILE` in the copied `wrangler.jsonc`, or set the same names as **build variables**. Build variables override the file. Runtime dashboard variable edits alone do not configure the deployment script.

Keep the generated D1 ID and R2 name in `wrangler.jsonc`. Each build reconstructs `wrangler.operator.json` from those bindings and the stored policies. Repeat deployments preserve disabled namespaces and storage budgets. A namespace cannot be reassigned to a different repository. Repository transfers or default-branch changes require an explicit `operator bind`, as described below. Use the CLI path for custom domains.

The automatic checks verify the new deployment ID, disabled HTTP caching, rejected anonymous uploads, invalid requests, and cache misses. They do not upload data or prove authorized GitHub OIDC writes. The [e2e plan](e2e-plan.md#deploy-to-cloudflare-button) covers the full acceptance checks.

The button temporarily targets `feat/public-remote-cache` for PR #718's live e2e test. Before marking the PR ready for review, restore the buttons in the root README, package README, and this guide to `main`, and remove this note. The CI button check rejects temporary branch URLs on a PR that is ready for review.

## CLI deployment

Use the following steps for direct deployment or custom domains. If you already deployed with the button, skip to [Connect your application build](#5-connect-your-application-build). To run later operator commands, clone the repository created by the button, install dependencies at its root, set the Cloudflare credentials below, and run `pnpm deploy` once to reconstruct its operator configuration. Run `pnpm operator` commands from that root.

## 1. Prepare the deployment checkout

Use a checkout of this repository that contains `packages/remote-cache`. Install Node.js 22.12 or newer and the pnpm version in the root `package.json`. From the repository root, run:

```sh
pnpm install --frozen-lockfile
pnpm check-remote-cache
cd packages/remote-cache
```

Run all subsequent `pnpm operator` commands from `packages/remote-cache`. This is the service checkout. Your application repository can be a separate checkout; it does not need the Worker source or Cloudflare credentials.

## 2. Prepare Cloudflare credentials

Start with a Cloudflare account on Workers Free and enable R2. A Workers Paid subscription is optional. [R2 has its own free usage allowance and billing](https://developers.cloudflare.com/r2/pricing/), separate from the Workers plan.

Find your account ID using [Cloudflare's account-ID instructions](https://developers.cloudflare.com/fundamentals/account/find-account-and-zone-ids/). This is the account ID, not a zone ID. Also find or create the account's [Workers subdomain](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/). For a URL ending in `my-team.workers.dev`, the subdomain is `my-team`.

[Create an API token](https://developers.cloudflare.com/fundamentals/api/get-started/create-token/) scoped to that account with these permissions:

| Account permission | Access                                                          |
| ------------------ | --------------------------------------------------------------- |
| Workers            | Admin at Workers product scope, to create and manage the Worker |
| D1                 | Edit / Write                                                    |
| Workers R2 Storage | Edit / Write                                                    |

These permissions cover deployment, database migrations, private bucket setup, and lifecycle rules. Accounts using the legacy token permissions can use Workers Scripts Edit / Write for Workers access. With the newer Workers roles, Editor only permits deployment to an existing Worker; initial setup requires product-level Admin. See [Workers roles](https://developers.cloudflare.com/workers/authorization/workers/) and the [API permission reference](https://developers.cloudflare.com/fundamentals/api/reference/permissions/) for the current labels.

Set `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` in the terminal that will run the operator. Enter the token at a hidden prompt so it does not enter shell history.

On macOS or Linux, in Bash or Zsh:

```sh
printf 'Cloudflare account ID: '
read -r CLOUDFLARE_ACCOUNT_ID
printf 'Cloudflare API token: '
read -r -s CLOUDFLARE_API_TOKEN
printf '\n'
export CLOUDFLARE_ACCOUNT_ID CLOUDFLARE_API_TOKEN
```

On Windows, in PowerShell:

```powershell
$env:CLOUDFLARE_ACCOUNT_ID = Read-Host 'Cloudflare account ID'
$token = Read-Host 'Cloudflare API token' -AsSecureString
$env:CLOUDFLARE_API_TOKEN = [System.Net.NetworkCredential]::new('', $token).Password
Remove-Variable token
```

The operator reads these environment variables directly. Putting them in `.dev.vars` or signing in with `wrangler login` alone does not configure the operator. Keep these credentials in the deployment terminal or deployment automation. Application build jobs use GitHub OIDC instead.

## 3. Deploy and bind your repository

Choose the following values. This example uses the public repository `acme/web-app` and a Workers subdomain of `my-team`:

| Option        | Example                                        | Meaning                                                         |
| ------------- | ---------------------------------------------- | --------------------------------------------------------------- |
| `--name`      | `acme-build-cache`                             | Name for the Worker, D1 database, and R2 bucket                 |
| `--namespace` | `web-app`                                      | Repository's cache namespace                                    |
| `--repo`      | `acme/web-app`                                 | GitHub repository allowed to upload; omit `https://github.com/` |
| `--origin`    | `https://acme-build-cache.my-team.workers.dev` | Public service origin, without `/projects/...`                  |

Use dedicated resource names. Replace the example repository and subdomain with your own values, then run:

```sh
pnpm operator setup --name acme-build-cache --namespace web-app --repo acme/web-app --origin https://acme-build-cache.my-team.workers.dev
```

Setup creates or reuses the named resources, applies the schema, keeps R2 private, registers the repository's IDs and default branch, and deploys the Worker. The endpoint it prints is:

```text
https://acme-build-cache.my-team.workers.dev/projects/web-app
```

Use that complete endpoint, without a trailing slash, in your client. It is also the exact audience required for upload tokens. The namespace does not have to match the repository name.

Setup writes `wrangler.operator.json`, an ignored file containing resource IDs and deployment settings. Keep it for later operator commands and back it up. It contains no API token. Repeating setup preserves an existing repository binding and does not re-enable a disabled namespace. Use a separate service checkout and configuration file for each deployment.

Omitting `--profile` selects `free`, with 16 generations per cleanup run. Defaults are seven days of retention, an 8 GB byte budget, 20,000 entries, and 20,000 secondary-key associations. Pending and retired uploads also consume the byte budget. To choose different limits during setup, add options such as:

```text
--retention-days 30 --byte-limit 30000000000 --entry-limit 50000 --association-limit 50000
```

### Select Paid when needed

Measure your deployed workload before choosing higher allowances. Workers Free currently allows [10 ms of CPU per invocation](https://developers.cloudflare.com/workers/platform/limits/). The full 4 MiB value and 64 MiB blob limits have not been validated against that CPU allowance. Local benchmark results cannot establish Cloudflare's billed CPU usage. If your workload exceeds Free limits, lower payload limits or choose Workers Paid in Cloudflare.

After choosing Workers Paid, you can rerun your setup command with `--profile paid` to process up to 256 generations per cleanup run. Preserve your deployment, namespace, repository, and origin arguments. The flag changes cleanup batch size only; it does not purchase a subscription or raise storage and retention budgets. You can keep the smaller `free` cleanup profile on a Paid account.

### Use a custom domain

Choose the custom origin during initial setup. For example:

```sh
pnpm operator setup --name acme-build-cache --namespace web-app --repo acme/web-app --origin https://cache.example.com
```

The domain must be in an active Cloudflare zone in the same account. Add Workers Routes write access and Zone read access for that zone to the deployment token. The operator configures a Worker Custom Domain and disables the unused `workers.dev` alias. The resulting endpoint is `https://cache.example.com/projects/web-app`. See [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/) and [route permissions](https://developers.cloudflare.com/workers/authorization/workers/).

Existing namespaces retain their original origin. Repeating setup with a different origin is rejected. Plan an origin change as a separate deployment and update clients to its new endpoint.

## 4. Check the service

Inspect the deployed repository binding and storage state:

```sh
pnpm operator status
```

In `scopes`, confirm that `scope_id` is `web-app`, `repository` is your repository, `branch` matches its default branch, and `endpoint` matches the printed URL. `enabled` and `writes_enabled` should both be `1` for a new namespace. The deployment-level switches must also be enabled.

Check that an anonymous upload is rejected:

```sh
curl --include --request POST https://acme-build-cache.my-team.workers.dev/projects/web-app/store
```

Expect HTTP `401` with the plain-text body `Invalid credentials`. Use `curl.exe` in Windows PowerShell. The service has no landing page at `/`; a browser visit there returns `404`.

An empty cache also returns `404` for a valid lookup until an authorized client publishes an entry. A complete integration check must upload from a permitted GitHub Actions job, fetch the same entry without credentials, and compare the returned value and blob. The [HTTP protocol reference](../README.md#protocol) defines those requests. The [e2e plan](e2e-plan.md) describes the project's separate shared staging checks.

## 5. Connect your application build

A compatible client needs two settings: the complete namespace endpoint and whether it can upload. Developer machines and PR jobs can read without credentials. For uploads, the job must run in the bound public repository on a `push` to its registered default branch. Grant that job `id-token: write`; GitHub documents this [OIDC permission and custom audiences](https://docs.github.com/en/actions/reference/security/oidc).

The client requests a GitHub OIDC token whose audience is the endpoint, then sends the token as `Authorization: Bearer <token>` to `/store`. `GITHUB_TOKEN`, personal access tokens, and Cloudflare API tokens are not accepted as cache upload credentials. PR, tag, and `workflow_dispatch` tokens cannot upload. A default branch named `master` works when that is the branch saved by setup.

### Proposed Vite+ configuration — client implementation required

The following examples describe the [RFC's client configuration](../rfcs/0001-remote-cache.md#client-configuration-and-remote-cache-modes). They do **not** enable remote caching in the client code currently in this repository.

Once a compatible `vp run` client implements that contract, merge this setting into your application's `vite.config.ts`:

```ts
export default {
  run: {
    remoteCache: {
      url: 'https://acme-build-cache.my-team.workers.dev/projects/web-app',
    },
  },
};
```

The proposed default is public reads when an endpoint is configured. `VP_REMOTE_CACHE_URL` overrides the endpoint. To enable uploads, add these settings to the existing build job that runs only on default-branch pushes:

```yaml
permissions:
  contents: read
  id-token: write
env:
  VP_REMOTE_CACHE_URL: https://acme-build-cache.my-team.workers.dev/projects/web-app
  VP_REMOTE_CACHE: read-write
```

Keep the job's checkout, dependency installation, and `vp run` build steps. For PR builds, use `VP_REMOTE_CACHE: read` and omit `id-token: write`. The RFC also proposes `VP_REMOTE_CACHE: off` to disable remote caching. The URL is public and can be stored as a GitHub repository variable. No Cloudflare secrets or GitHub environment are required for application cache access.

## Add or update repository bindings

One deployment can serve multiple repositories. Add another public repository with a separate namespace:

```sh
pnpm operator bind --namespace shared-ui --repo acme/shared-ui
```

This redeploys the namespace configuration and prints `https://acme-build-cache.my-team.workers.dev/projects/shared-ui`. Configure that repository's client with its own endpoint.

After renaming or transferring a repository, or changing its default branch, review and refresh its existing binding:

```sh
pnpm operator bind --namespace web-app --repo acme/web-app
```

Use the new repository path after a rename or transfer. `bind` verifies the immutable repository ID and refreshes the owner and branch. To bind an unrelated repository, choose a new namespace.

To pause uploads or withdraw public access:

```sh
pnpm operator policy --namespace web-app --writes off
pnpm operator policy --namespace web-app --enabled off
```

Making the GitHub repository private does not withdraw previously published cache data. Use the operator to disable or purge the namespace. The [operations reference](../README.md#operations) covers retention changes, upgrades, backups, purge, and teardown.

## Troubleshooting

| Symptom                               | Check                                                                                                |
| ------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Setup cannot find the repository      | `--repo` must be an existing public GitHub.com `owner/repository`                                    |
| Cloudflare API returns `403`          | Token permissions, account scope, and R2 activation                                                  |
| Namespace requests return `404`       | Exact endpoint, namespace, and scope/deployment switches; fetch misses are also `404`                |
| `/store` returns `401`                | Missing, invalid, or expired GitHub OIDC token                                                       |
| `/store` returns `403`                | Token audience, repository/owner IDs, registered branch, `push` event, and write switch              |
| `/store` returns `503`                | Quotas, concurrent uploads, storage availability, and policy changes; inspect `pnpm operator status` |
| `vp run` does not contact the service | This source tree has no remote-cache client adapter; the RFC settings alone cannot add it            |

The `REMOTE_CACHE_*` variables in this project's staging workflow configure its own test deployment. They are not application-client settings. Use the dedicated resources and namespace you created above for your repository.
