// Asks real Obsidian which tags it finds in the fixture vault in
// crates/orbit-core/tests/fixtures/tags, for tags.obsidian.json.
//
// Run Obsidian on a copy of the vault, apart from your own Obsidian:
//   cp -r crates/orbit-core/tests/fixtures/tags /tmp/tags-vault
//   mkdir -p /tmp/obsidian-data && echo '{"vaults":{"fixture":{"path":"/tmp/tags-vault","ts":1,"open":true}}}' \
//     > /tmp/obsidian-data/obsidian.json
//   electron /usr/lib/obsidian/app.asar --user-data-dir=/tmp/obsidian-data --remote-debugging-port=9333 &
//   node scripts/obsidian-tags.mjs > out.json
// then keep each note's inline tag names (`notes[path].inline[].tag`) as `inline`, and
// `counts`, in tags.obsidian.json.
const targets = await (await fetch("http://127.0.0.1:9333/json")).json();
const page = targets.find(t => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise(r => ws.onopen = r);
function evaluate(expression) {
  return new Promise((resolve) => {
    ws.addEventListener("message", function on(ev) {
      const msg = JSON.parse(ev.data);
      if (msg.id === 1) { ws.removeEventListener("message", on); resolve(msg.result); }
    });
    ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate",
      params: { expression, returnByValue: true, awaitPromise: true } }));
  });
}
const script = `(async () => {
  for (let i = 0; i < 100 && !(window.app && app.metadataCache
       && app.metadataCache.getFileCache(app.vault.getFileByPath("Inline.md") || {})); i++)
    await new Promise(r => setTimeout(r, 200));
  await new Promise(r => setTimeout(r, 1000));
  const notes = {};
  for (const f of app.vault.getMarkdownFiles().sort((a, b) => a.path < b.path ? -1 : 1)) {
    const c = app.metadataCache.getFileCache(f) || {};
    notes[f.path] = {
      frontmatter: c.frontmatter ? (c.frontmatter.tags ?? c.frontmatter.tag ?? null) : null,
      inline: (c.tags || []).map(t => ({ tag: t.tag, start: t.position.start.offset, end: t.position.end.offset })),
    };
  }
  return JSON.stringify({ version: app.getVersion ? app.getVersion() : navigator.userAgent,
    notes, counts: app.metadataCache.getTags() });
})()`;
const result = await evaluate(script);
console.log(result.result ? result.result.value : JSON.stringify(result));
ws.close();
