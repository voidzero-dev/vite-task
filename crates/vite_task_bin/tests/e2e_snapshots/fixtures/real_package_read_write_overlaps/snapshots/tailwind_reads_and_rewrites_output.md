# tailwind_reads_and_rewrites_output

Tailwind's CLI stats and reads an existing output file to compare its contents with the newly generated CSS, then writes the file when the contents differ. This makes `dist/output.css` both a read and a write in one command. See [`outputFile`](https://github.com/tailwindlabs/tailwindcss/blob/8a14a710102cae195f6811e8578bef9477bc6be9/packages/%40tailwindcss-cli/src/commands/build/utils.ts#L15-L34).

## `vt run -v overlap`

```
~/cases/tailwind-output$ tailwindcss -i input.css -o dist/output.css --minify
≈ tailwindcss v4.3.1

Done in <duration>


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] tailwind-output-overlap#overlap: ~/cases/tailwind-output$ tailwindcss -i input.css -o dist/output.css --minify ✓
      → Not cached: read and wrote 'cases/tailwind-output/dist/output.css'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```
