import { useEffect, useMemo, useState } from 'react'
import { Connection, LAMPORTS_PER_SOL } from '@solana/web3.js'
import { useWallet } from '@solana/wallet-adapter-react'
import { CircleCheck, CircleX, ExternalLink, Loader2 } from 'lucide-react'

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Separator } from '@/components/ui/separator'
import { WalletButton } from '@/components/wallet-button'
import {
  NETWORKS,
  OPS,
  PROGRAM_ID,
  ProgramError,
  calculate,
  expected,
  explorerTxUrl,
  isDeployed,
  parseU64,
  type Network,
  type Op,
} from '@/lib/calculator'

type Entry = {
  id: number
  text: string
  network: Network
} & ({ ok: true; value: bigint; signature: string } | { ok: false; error: string; code?: number })

const opOf = (op: Op) => OPS.find((o) => o.value === op)!

export default function App() {
  const { publicKey, signTransaction } = useWallet()
  const [network, setNetwork] = useState<Network>('devnet')
  const connection = useMemo(() => new Connection(NETWORKS[network].rpc, 'confirmed'), [network])

  // true / false, null = RPC unreachable, undefined = still checking
  const [deployed, setDeployed] = useState<boolean | null | undefined>(undefined)
  const [balance, setBalance] = useState<number | null>(null)
  const [op, setOp] = useState<Op>(0)
  const [a, setA] = useState('40')
  const [b, setB] = useState('2')
  const [busy, setBusy] = useState(false)
  const [history, setHistory] = useState<Entry[]>([])

  // Is the program on this network? (null = can't reach the RPC, e.g. surfpool not running)
  useEffect(() => {
    setDeployed(undefined)
    isDeployed(connection).then(setDeployed, () => setDeployed(null))
  }, [connection])

  const refreshBalance = () => {
    if (!publicKey) return setBalance(null)
    connection.getBalance(publicKey).then((l) => setBalance(l / LAMPORTS_PER_SOL), () => setBalance(null))
  }
  useEffect(refreshBalance, [connection, publicKey])

  // Live preview of what the program should answer, using the same checked u64 math.
  let preview: string
  let inputError: string | null = null
  try {
    const x = parseU64(a, 'a')
    const y = parseU64(b, 'b')
    const r = expected(op, x, y)
    preview = r === null ? (op === 3 && y === 0n ? 'will fail: division by zero' : 'will fail: overflow') : `= ${r}`
  } catch (e) {
    inputError = (e as Error).message
    preview = ''
  }

  async function run() {
    if (!publicKey || !signTransaction || inputError) return
    const x = parseU64(a, 'a')
    const y = parseU64(b, 'b')
    const text = `${x} ${opOf(op).symbol} ${y}`
    setBusy(true)
    try {
      const out = await calculate(connection, publicKey, signTransaction, op, x, y)
      setHistory((h) => [{ id: Date.now(), text, network, ok: true, value: out.value, signature: out.signature }, ...h])
    } catch (e) {
      const err = e as Error
      setHistory((h) => [
        { id: Date.now(), text, network, ok: false, error: err.message, code: e instanceof ProgramError ? e.code : undefined },
        ...h,
      ])
    } finally {
      setBusy(false)
      refreshBalance()
    }
  }

  return (
    <div className="min-h-svh bg-background text-foreground">
      <header className="mx-auto flex max-w-2xl items-center justify-between gap-4 px-4 py-6">
        <div>
          <h1 className="font-heading text-lg font-semibold">sBPF Calculator</h1>
          <p className="font-mono text-xs text-muted-foreground">{PROGRAM_ID.toBase58()}</p>
        </div>
        <WalletButton />
      </header>

      <main className="mx-auto grid max-w-2xl gap-6 px-4 pb-16">
        <Card>
          <CardHeader>
            <CardTitle>Calculate on-chain</CardTitle>
            <CardDescription>
              Checked u64 math in hand-written sBPF assembly. Each call creates a fresh 8-byte
              result account (~0.001 SOL rent). Rejected calls are caught in simulation and cost
              nothing.
            </CardDescription>
          </CardHeader>

          <CardContent className="grid gap-5">
            <div className="grid gap-2">
              <Label>Network</Label>
              <div className="flex flex-wrap items-center gap-3">
                <Select value={network} onValueChange={(v) => setNetwork(v as Network)}>
                  <SelectTrigger className="w-48">
                    <SelectValue>{NETWORKS[network].label}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {(Object.keys(NETWORKS) as Network[]).map((n) => (
                      <SelectItem key={n} value={n}>
                        {NETWORKS[n].label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {deployed === true && <Badge variant="secondary">program deployed</Badge>}
                {deployed === false && <Badge variant="destructive">not deployed here</Badge>}
                {balance !== null && (
                  <span className="text-sm text-muted-foreground tabular-nums">{balance.toFixed(4)} SOL</span>
                )}
              </div>
              {network === 'local' && deployed === null && (
                <p className="text-sm text-muted-foreground">
                  Can't reach 127.0.0.1:8899. Start surfpool:{' '}
                  <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">surfpool start --network devnet</code>
                </p>
              )}
              {deployed === false && (
                <p className="text-sm text-muted-foreground">
                  Deploy first:{' '}
                  <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">sbpf deploy calculator {network}</code>
                </p>
              )}
            </div>

            <Separator />

            <div className="grid grid-cols-[1fr_auto_1fr] items-end gap-3">
              <div className="grid gap-2">
                <Label htmlFor="a">a</Label>
                <Input id="a" inputMode="numeric" className="font-mono tabular-nums" value={a} onChange={(e) => setA(e.target.value)} />
              </div>
              <div className="grid gap-2">
                <Label>op</Label>
                <Select value={op} onValueChange={(v) => setOp(v as Op)}>
                  <SelectTrigger className="w-24">
                    <SelectValue>{`${opOf(op).symbol} ${opOf(op).name}`}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {OPS.map((o) => (
                      <SelectItem key={o.value} value={o.value}>
                        {o.symbol} {o.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-2">
                <Label htmlFor="b">b</Label>
                <Input id="b" inputMode="numeric" className="font-mono tabular-nums" value={b} onChange={(e) => setB(e.target.value)} />
              </div>
            </div>

            <p className={`min-h-5 font-mono text-sm ${inputError ? 'text-destructive' : 'text-muted-foreground'}`}>
              {inputError ?? preview}
            </p>
          </CardContent>

          <CardFooter className="justify-end">
            <Button onClick={run} disabled={!publicKey || !!inputError || busy || deployed !== true}>
              {busy && <Loader2 className="animate-spin" />}
              {!publicKey ? 'Connect a wallet first' : busy ? 'Sending…' : 'Send to the program'}
            </Button>
          </CardFooter>
        </Card>

        {history.length > 0 && (
          <section className="grid gap-3">
            <h2 className="text-sm font-medium text-muted-foreground">History</h2>
            {history.map((h) => (
              <Alert key={h.id} variant={h.ok ? 'default' : 'destructive'}>
                {h.ok ? <CircleCheck /> : <CircleX />}
                <AlertTitle className="font-mono">
                  {h.text} {h.ok ? `= ${h.value}` : ''}
                </AlertTitle>
                <AlertDescription>
                  {h.ok ? (
                    explorerTxUrl(h.network, h.signature) ? (
                      <a className="inline-flex items-center gap-1 underline underline-offset-4" href={explorerTxUrl(h.network, h.signature)!} target="_blank" rel="noreferrer">
                        View transaction <ExternalLink className="size-3" />
                      </a>
                    ) : (
                      <span className="font-mono text-xs break-all">{h.signature}</span>
                    )
                  ) : (
                    <span>Rejected: {h.error}</span>
                  )}
                </AlertDescription>
              </Alert>
            ))}
          </section>
        )}
      </main>
    </div>
  )
}
