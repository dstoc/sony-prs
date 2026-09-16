# Syntax-highlighted code

The reader keeps source newlines, wraps long commands, and carries syntax
state across every displayed line and page.

```rust
fn main() {
    let message = "hello from an agent";
    println!("{message}");
}
```

```python
def load(path):
    '''A string that intentionally continues
    over a source newline.'''
    return {"path": path, "ready": True}
```

```bash
printf '%s\n' "this shell command is deliberately very long so that a narrow page wraps it without horizontal scrolling and keeps every character visible"
```

```unknown-agent-language
plain output { still: readable, even: when_unknown: true }
```

```diff
@@ -1,2 +1,2 @@
-old value
+new value
```

```json
{"name": "reader", "pages": [1, 2, 3]}
```

```yaml
reader:
  mode: eink
  syntax: bounded
```

```toml
[reader]
enabled = true
```

```sql
SELECT page FROM reading_location WHERE page > 1;
```

```cpp
int main() { return 0; }
```

```go
func main() { println("ready") }
```

```html
<main class="reader">Hello</main>
```

```css
.reader { display: block; color: #111; }
```

```javascript
const pages = await reader.nextPage();
```

```typescript
const page: number = await reader.nextPage();
```

```c
/* This comment starts on one line
   and ends after the page may have changed. */
int ready = 1;
```

```markdown
## A nested heading

[a link](https://example.invalid)
```

```rust
// This block is intentionally longer than a page.
fn repeated(value: usize) -> usize { value + 1 }
fn another(value: usize) -> usize { repeated(value) }
fn third(value: usize) -> usize { another(value) }
fn fourth(value: usize) -> usize { third(value) }
fn fifth(value: usize) -> usize { fourth(value) }
fn sixth(value: usize) -> usize { fifth(value) }
fn seventh(value: usize) -> usize { sixth(value) }
fn eighth(value: usize) -> usize { seventh(value) }
```
