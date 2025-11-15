```json
{
  "scripts": {
    "build": "vite build",
    "lint": "eslint src/",
    "test": "jest"
  },
  "cache": false,
  "inputs": [
    "src/**",
    "!src/**/*.test.ts",
    "package.json",
    "vite.config.ts"
  ],
  "outputs": [
    "dist/**"
  ]
}
```
