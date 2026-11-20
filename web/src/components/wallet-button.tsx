import { useState } from 'react'
import { WalletReadyState } from '@solana/wallet-adapter-base'
import { useWallet } from '@solana/wallet-adapter-react'
import { LogOut, Wallet } from 'lucide-react'

import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

const short = (s: string) => `${s.slice(0, 4)}…${s.slice(-4)}`

/** Connect / disconnect button. Lists every installed wallet (Phantom, Solflare, Backpack…). */
export function WalletButton() {
  const { wallets, wallet, publicKey, select, connect, disconnect, connecting } = useWallet()
  const [open, setOpen] = useState(false)

  if (publicKey) {
    return (
      <Button variant="outline" onClick={() => disconnect()}>
        {wallet && <img src={wallet.adapter.icon} alt="" className="size-4" />}
        <span className="font-mono">{short(publicKey.toBase58())}</span>
        <LogOut />
      </Button>
    )
  }

  const installed = wallets.filter((w) => w.readyState === WalletReadyState.Installed)

  return (
    <>
      <Button onClick={() => setOpen(true)} disabled={connecting}>
        <Wallet />
        {connecting ? 'Connecting…' : 'Connect wallet'}
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>Connect a wallet</DialogTitle>
            <DialogDescription>
              Any network works for signing: the app sends the transaction to the network you
              picked.
            </DialogDescription>
          </DialogHeader>
          {installed.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No wallet found. Install Phantom, Solflare or Backpack, then reload.
            </p>
          ) : (
            <div className="grid gap-2">
              {installed.map((w) => (
                <Button
                  key={w.adapter.name}
                  variant="outline"
                  className="h-11 justify-start"
                  onClick={async () => {
                    select(w.adapter.name)
                    setOpen(false)
                    // select() is async under the hood; connect on the next tick
                    setTimeout(() => connect().catch(() => {}), 0)
                  }}
                >
                  <img src={w.adapter.icon} alt="" className="size-5" />
                  {w.adapter.name}
                </Button>
              ))}
            </div>
          )}
        </DialogContent>
      </Dialog>
    </>
  )
}
