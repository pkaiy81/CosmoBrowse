# JS Event Loop (minimum runtime)

`CosmoBrowse` の最小 JS ランタイムは、HTML の event loop モデルを簡略化して次の順で処理します。

- task queue を 1 件実行
- その直後に microtask queue を空になるまで実行
- 次の task へ進む

```text
+--------------------+
| Parse HTML + JS    |
+---------+----------+
          |
          v
+--------------------+
| Execute top-level  |
| script statements  |
+---------+----------+
          |
          v
+-------------------------------+
| Event loop tick               |
| 1) pop task (setTimeout/click)|
| 2) run callback               |
| 3) drain microtasks           |
+---------+---------------------+
          |
          v
+-------------------------------+
| If DOM mutated:               |
| relayout -> repaint           |
+-------------------------------+
```

## Supported minimum set

- click handler
  - `element.onclick = handler`
  - `element.addEventListener("click", handler)`
- timer
  - `setTimeout(callback, delay)` (`delay` は現在無視)
- microtask
  - `queueMicrotask(callback)`
- DOM read/write
  - `document.getElementById(id)`
  - `element.textContent`

## Unsupported API error policy

未対応 API は次の統一形式で diagnostics に記録します。

- `Unsupported browser API: <api_name>`

## Spec references

- HTML Standard: event loop processing model
  - https://html.spec.whatwg.org/multipage/webappapis.html#event-loop-processing-model
- DOM Standard: event dispatch
  - https://dom.spec.whatwg.org/#concept-event-dispatch
- ECMAScript: execution contexts
  - https://262.ecma-international.org/#sec-execution-contexts
