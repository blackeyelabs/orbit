# Split view is removed; Live Preview is the default

Each tab shows a note in Live Preview, the default, or Source mode, and Ctrl+E switches between them. Split view (source beside a rendered preview) is gone, and the rendered preview's code with it.

The first plan kept Split view as the default until Live Preview passed its gate (#56), behind a development setting, so the app never got worse mid-way. On 2026-11-06 the user chose to remove Split view early instead: one rich view to design, test and keep fast, used daily while it's built. Until each block's Live Preview ticket lands, blocks it doesn't render yet (lists, quotes, code, tables, images, callouts, math, Mermaid) show as their source; the old preview's plugins are in git history for those tickets to reuse.

A reader might expect Split view to stay as a third option, as it costs little to keep. It's dropped so the rendering code has one owner (Live Preview's block widgets) instead of two that drift apart.
