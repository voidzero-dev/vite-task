# Vite task TUI Design

```json
{
  "script": {
    "ready": "tsc --noEmit && vite lint && vite run -r build"
  }
}
```

<table>
  <tbody>
    <tr style="vertical-align: top">
      <td>
        <ul>
          <li><code>vite run ready</code>
            <ul>
              <li><code>tsc --noEmit</code></li>
              <li><code>vite lint</code></li>
              <li><code>vite run -r build</code>
              <ul>
              <li><code>pkg1#build</code></li>
              <li><code>pkg2#build</code></li>
              <li><code>pkg3#build</code></li>
          </li>
        </ul>
      </td>
      <td>
      <pre>
$ vite build <br />
VITE+ v1.0.0 building for production
transforming...
✓ 32 modules transformed...
rendering chunks...
computing gzip size...
dist/index.html  0.46 kB | gzip: 0.30 kB
dist/assets/react-CHdo91hT.svg  4.13 kB | gzip: 2.05 kB
dist/assets/index-D8b4DHJx.css  1.39 kB | gzip: 0.71 kB
dist/assets/index-CAl1KfkQ.js188.06 kB | gzip: 59.21 kB
✓ built in 308ms
</pre>
      </td>
    </tr>

</tbody>
</table>
