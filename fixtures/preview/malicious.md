# Plan <script>window.pwned = 1</script>

<script>window.pwned = 2</script>

<img src="x" onerror="window.pwned = 3">

<iframe src="https://evil.example/"></iframe>

<div onclick="window.pwned = 4">raw html block</div>

[js link](javascript:alert(1))
[data link](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)
[file link](file:///etc/passwd)
[web link](https://example.com/page)
[relative link](./other.md)
[anchor link](#details)

![tracking pixel](https://evil.example/pixel.png)

## Details

```mermaid
graph TD
  A[Start] --> B[End]
  click A callback "tooltip"
  click B "javascript:alert(1)"
```

| a | b |
|---|---|
| 1 | <b onmouseover="x()">2</b> |
