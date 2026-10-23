# Sweepwise Help (the website's support chat)

A small Cloudflare Worker. It answers questions about Sweepwise from `src/kb.js` using an OpenAI-compatible AI (DeepSeek first), and, only if the visitor says yes, files a short redacted summary as a public GitHub issue and pings the maker on Discord. It stores no messages: the only things kept are counters (a hashed visitor id per day and a daily total) for the spending caps.

Tests: `cd worker && node --test` (a fake AI, GitHub and Discord are used; nothing real is contacted).

## One-time setup (Shad does these; the keys are typed by you and never shared with anyone)

1. Make a free account at cloudflare.com.
2. In a terminal in this `worker` folder: `npx wrangler@3 login`, then `npx wrangler@3 kv namespace create RATE`. Paste the id it prints into `wrangler.toml` where it says `PASTE_THE_KV_ID_HERE`.
3. Make a DeepSeek account at platform.deepseek.com, add a small prepaid balance (US$5 lasts a very long time) and create an API key.
4. Make a GitHub fine-grained token (GitHub > Settings > Developer settings): repository `AlbertanCoder/sweepwise` only, permission **Issues: Read and write**, nothing else.
5. In a private Discord channel make a webhook (Channel settings > Integrations > Webhooks).
6. Store the three secrets (each command asks you to paste the value; it is never written to a file):
   - `npx wrangler@3 secret put AI_API_KEY`
   - `npx wrangler@3 secret put GITHUB_TOKEN`
   - `npx wrangler@3 secret put DISCORD_WEBHOOK_URL`
7. `npx wrangler@3 deploy`. It prints the Worker's address (like `https://sweepwise-help.<you>.workers.dev`).
8. Tell Claude the address. It goes into `docs/index.html` as `data-endpoint="..."`, the website is checked, and the `support-bot` branch is merged.

## Changing the AI later

Edit `API_BASE` and `MODEL` in `wrangler.toml` and run `npx wrangler@3 deploy`. Any OpenAI-compatible service works (for example Anthropic's OpenAI-compatible endpoint with a Claude model). Change the `AI_API_KEY` secret to match.

## Cost control

`DAILY_MESSAGE_CAP` (default 1200 messages a day, about US$1 on DeepSeek) and `PER_VISITOR_CAP` (20 a day). When the daily total is reached the chat says it is resting and points to "Report a problem". Set a low spending limit on the DeepSeek balance too.

## Keeping the answers right

`src/kb.js` is the only source the bot may use. When the app or the website changes, update that file and redeploy, or the bot will repeat old facts.
