# one_line_when_captured

## `vtt pipe-stdout -- vt run --json`

```
--- stdout ---
{"$schema":"<schema>","version":1,"workspaceRoot":"<workspace>","packages":{".":{"name":"app"},"packages/lib":{"name":"lib"}},"tasks":{"build":{"package":".","name":"build","command":"vtt print built","dependsOn":["packages/lib#build"]},"packages/lib#build":{"package":"packages/lib","name":"build","command":"vtt print built lib"},"packages/lib#test":{"package":"packages/lib","name":"test","cwd":"packages","command":"vtt print tested lib"}}}
```
