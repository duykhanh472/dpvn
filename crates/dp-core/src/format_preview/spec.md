# Format preview markup — locked specification

Status: **locked** (Phase 9, resolves open question Q1 by adopting
Appendix B of the implementation plan).

This is the exact markup subset accepted by `dp-core::format_preview`. A
post-processor may rely on the rendered HTML shape described here. Anything
not listed renders literally (after HTML escaping).

## Block markup

| Markup | Output |
|---|---|
| `_text_` | `<i>text</i>` |
| `*text*` | `<b>text</b>` |
| `=_text_=` | `<b><i>text</i></b>` |
| `<sc>text</sc>` | `<span class="sc">TEXT</span>` |
| `# text` at line start | `<blockquote><p>text</p></blockquote>` |
| `/>` at line start | poetry line (leading spaces preserved) |
| `[Footnote 1: text]` | footnote reference plus a list at the end |
| `----` alone on a line (4+ hyphens) | `<hr>` |
| Blank line | paragraph break |

Consecutive `#` lines form one `<blockquote>` with one `<p>` per line.
Consecutive `/>` lines form one `<div class="poetry">` with one
`<div class="poem-line">` per line. Consecutive ordinary lines form one
paragraph; the line break is treated as a space by the browser.

## Inline markup

* Inline markers may span a physical line inside a paragraph.
* Nesting is allowed and capped at **8** levels. Markup opened beyond the cap
  renders literally.
* `_` does not open emphasis inside a word, so `snake_case` survives.
* Unclosed markers render literally and never swallow the rest of the page.
* A malformed footnote (no closing `]`) renders literally.
* Vietnamese letters and diacritics are never markup.

## Footnote output

```html
<sup class="fnref" id="fnref-1"><a href="#fn-1">1</a></sup>
...
<ol class="footnotes">
  <li id="fn-1" value="1">
    <span class="fn-text">Nội dung ghi chú</span>
    <a class="fn-back" href="#fnref-1" aria-label="Quay lại ghi chú">↩</a>
  </li>
</ol>
```

Footnote numbers are taken from the source text. The first occurrence of a
number wins; repeated references reuse the same list entry.

## Safety

Every byte of page text is HTML-escaped before markup is added. The assembled
fragment is then passed through `ammonia` with an explicit tag/attribute
allowlist. Page text can never inject scripts, event handlers, or styles.
