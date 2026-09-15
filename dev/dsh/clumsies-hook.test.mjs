import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, unlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { resolveWorkspace } from './clumsies-hook.mjs'

const root = mkdtempSync(join(tmpdir(), 'clumsies-dsh-global-'))
try {
  const runtime = '/Applications/Clumsies.app/Contents/Resources/clumsiesd'
  mkdirSync(join(root, '.dsh'))
  const config = join(root, '.dsh/clumsies.json')
  writeFileSync(config, JSON.stringify({ runtime }))
  for (const workspace of ['/repos/one', '/repos/two/nested']) {
    assert.deepEqual(resolveWorkspace(workspace, root), { workspace, runtime })
  }
  writeFileSync(config, JSON.stringify({ runtime: '/tmp/foreign-runtime' }))
  assert.equal(resolveWorkspace('/repos/one', root), null)
  unlinkSync(config)
  assert.equal(resolveWorkspace('/repos/one', root), null, 'Disabling the global adapter stops forwarding')
} finally {
  rmSync(root, { recursive: true, force: true })
}
