# Split view is removed when Live Preview passes its gate

Once Live Preview renders and edits every block today's preview renders, plus the properties panel, it becomes the only rich view: each tab switches between Live Preview and Source mode (Ctrl+E), and Split view (source beside a rendered preview) is removed in that same release. Until then Live Preview is built behind a development setting and Split view stays the default, so the daily-usable app never gets worse mid-way.

A reader might expect Split view to stay as a third option, as it costs little to keep. It's dropped so there is one rich view to design, test and keep fast, and so the preview's rendering code has one owner (Live Preview's block widgets) instead of two that drift apart.
