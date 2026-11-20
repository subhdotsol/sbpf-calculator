# calculator web

Browser UI for the sBPF calculator (`../src/calculator/calculator.s`), using bun + Vite + React + Tailwind v4 + shadcn/ui.

```sh
bun install
bun dev
```

- `src/lib/calculator.ts`: encoding, checked math preview, simulate → sign → send → read result
- `src/components/wallet-button.tsx`: wallet picker (shadcn Dialog, wallet-standard wallets)
- `src/App.tsx`: network select, inputs, history
- `src/components/ui/`: shadcn components (`bunx --bun shadcn@latest add <name>` for more)

See `../README.md` → "The web app" for details.
