// Asks real Obsidian how it resolves and parses the links of the fixture
// vault in crates/orbit-core/tests/fixtures/links, for links.obsidian.json.
//
// Run Obsidian on a copy of the vault, apart from your own Obsidian:
//   cp -r crates/orbit-core/tests/fixtures/links /tmp/links-vault
//   mkdir -p /tmp/obsidian-data && echo '{"vaults":{"fixture":{"path":"/tmp/links-vault","ts":1,"open":true}}}' \
//     > /tmp/obsidian-data/obsidian.json
//   electron /usr/lib/obsidian/app.asar --user-data-dir=/tmp/obsidian-data --remote-debugging-port=9333 &
//   node scripts/obsidian-links.mjs > out.json
// then copy `resolution` and `parsed` from out.json into links.obsidian.json.
const sources = ["Home.md", "Inbox/Capture.md", "Projects/Deep/Notes.md", "Daily/2024-01-01.md",
  "Inboxes/Ideas.md", "Long/Way/Down/Same.md", "notes.txt"];
const links = ["Ideas", "ideas", "IDEAS.md", "Plan", "Same", "Projects/Plan", "Plan.md", "Deep/Ideas",
  "deep/ideas", "../Home", "./Ideas", "../Ideas", "diagram.png", "assets/diagram.png", "Roadmap",
  "The Plan", "Old plan", "Missing note", "My.Dotted.Name", "notes.txt", "notes", "/Ideas",
  "Inbox/Ideas", "Way/Down/Same", "Down/Same", "", "Folder/Same.md", "x/Ideas", "nbox/Ideas", "Home.md.md"];
const targets = await (await fetch("http://127.0.0.1:9333/json")).json();
const page = targets.find(t => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise(r => ws.onopen = r);
let id = 0;
function evaluate(expression) {
  return new Promise((resolve) => {
    const my = ++id;
    ws.addEventListener("message", function on(ev) {
      const msg = JSON.parse(ev.data);
      if (msg.id === my) { ws.removeEventListener("message", on); resolve(msg.result); }
    });
    ws.send(JSON.stringify({ id: my, method: "Runtime.evaluate",
      params: { expression, returnByValue: true, awaitPromise: true } }));
  });
}
const script = `(async () => {
  for (let i = 0; i < 100 && !(window.app && app.metadataCache && app.metadataCache.resolvedLinks
       && app.vault.getMarkdownFiles().length > 0 && app.metadataCache.getFileCache(app.vault.getFileByPath("Home.md"))); i++)
    await new Promise(r => setTimeout(r, 200));
  const sources = ${JSON.stringify(sources)}, links = ${JSON.stringify(links)};
  const resolution = [];
  for (const s of sources) for (const l of links) {
    const f = app.metadataCache.getFirstLinkpathDest(l, s);
    resolution.push({ from: s, link: l, to: f ? f.path : null });
  }
  const parsed = {};
  for (const f of app.vault.getMarkdownFiles()) {
    const c = app.metadataCache.getFileCache(f) || {};
    const pick = (x, embed) => ({ link: x.link, original: x.original, displayText: x.displayText,
      start: x.position.start.offset, end: x.position.end.offset, embed });
    parsed[f.path] = [...(c.links || []).map(x => pick(x, false)), ...(c.embeds || []).map(x => pick(x, true))]
      .sort((a, b) => a.start - b.start);
  }
  return JSON.stringify({ version: app.getVersion ? app.getVersion() : navigator.userAgent, resolution, parsed });
})()`;
const result = await evaluate(script);
console.log(result.result ? result.result.value : JSON.stringify(result));
ws.close();
