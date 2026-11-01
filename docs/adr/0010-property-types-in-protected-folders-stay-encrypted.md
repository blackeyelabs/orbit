# Property types set in protected folders stay encrypted

Property types (text, list, number, checkbox, date, date-time) are guessed from a value until the user sets one; a type the user sets is saved per vault, as Obsidian's `types.json` does. A type set while editing a note inside a protected folder is saved in that folder's encrypted storage instead, and used only while the folder is unlocked.

A single plain list of types would be simpler and match Obsidian exactly, but a key used only in protected notes (`diagnosis`, `salary`) would then sit in a plain file and give away what protection hides. Only types the user sets by hand are saved at all, and the rule is a line in the leak checklist.
