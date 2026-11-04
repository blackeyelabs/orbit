# Live Preview renders through Kit's editor_host

Live Preview hands Kit's `editor_host` a tree of block nodes, one keyed text leaf per paragraph, heading, list item, code block and table cell, and gets back each leaf's geometry. Orbit keeps everything else: the source text, the split into blocks and leaves, the conceal model's offset map per leaf, the caret and selection, Up/Down across blocks, virtualization, and platform input (IME).

Spike 4 (#36, `spikes/SPIKES.md`) passed its deciding checks this way, in headless tests through Kit's real layout: blocks rendering, the caret crossing blocks and cells, clicks in every block, dragging across blocks, typing into a cell, and IME geometry in a paragraph and a cell. With the rows virtualized and long lists split into runs, typing into a 200 KB note took about 6 ms a frame. The real IME (fcitx5 with Mozc) is the one deciding check only a person can run; until it passes, this decision is provisional.

The alternative was a custom element grown from Spike 1, shaping text with GPUI directly. It would own every pixel but redo what Kit already renders: heading sizes, lists, code highlighting and tables.

What Orbit takes on with this choice, from the spike:
- Virtualization is Orbit's: rows in GPUI's list, a long list split into runs of items, and moving the caret into rows that aren't laid out yet.
- Kit's `LeafGeometry` has no caret affinity at soft wraps, so Orbit tracks which line a caret at a wrap belongs to.
- `TextMark` has no text colour, so revealed syntax can't be dimmed, only tinted, until Kit adds one.
- Kit's API is new in 0.8.0, and Orbit pins it exactly, so an upgrade is a deliberate step.
