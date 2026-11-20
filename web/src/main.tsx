import { Buffer } from 'buffer'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { WalletProvider } from '@solana/wallet-adapter-react'

import './index.css'
import App from './App.tsx'

// @solana/web3.js v1 expects Node's Buffer to exist in the browser.
globalThis.Buffer ??= Buffer

// Follow the OS light/dark setting (shadcn's dark theme keys off the `.dark` class).
const dark = window.matchMedia('(prefers-color-scheme: dark)')
const applyTheme = () => document.documentElement.classList.toggle('dark', dark.matches)
applyTheme()
dark.addEventListener('change', applyTheme)

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {/* wallets={[]}: wallet-standard wallets (Phantom, Solflare, Backpack…) register themselves */}
    <WalletProvider wallets={[]} autoConnect>
      <App />
    </WalletProvider>
  </StrictMode>,
)
