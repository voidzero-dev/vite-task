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
    <tr>
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
        <pre style="height: 100%">terminal</pre>
      </td>
    </tr>

</tbody>
</table>
