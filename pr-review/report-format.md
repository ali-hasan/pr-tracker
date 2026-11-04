# PR review report format

Single source of truth for how a review report must look. Both the scheduled job (`prompt.md`, every
30 min) and a manual re-review (`review-one.sh`, triggered from the tracker app's "Review…" button)
point here instead of repeating this — if the format ever needs to change, change it here once, not
in two places that can drift apart.

## Line numbers — verify every single one, never compute or guess them

Citing a line number from counting lines in the raw diff text, or from a hunk's own
`@@ -a,b +c,d @@` header done by hand, produces a WRONG line number — it's easy to miscount across
hunks or deleted lines. This has gone wrong before, off by hundreds. Do not do either of those.

Instead, for every finding you're about to cite, fetch the real file at the PR's head commit and grep
for the exact line content to get its true line number:

```
gh api -H "Accept: application/vnd.github.raw" "repos/<owner>/<repo>/contents/<path>?ref=<headRefOid>" | grep -n "<unique snippet from the line>"
```

Only cite a `file:line` once this grep has confirmed it against the real file. If a finding's line
can't be confirmed this way (e.g. the line is new and the snippet isn't unique enough), widen the grep
pattern or describe the location by function name instead of guessing a number.

## Links — every citation is clickable, landing on the exact line

A bare `file:line` string makes me go search the diff by hand for it. Every citation must instead be a
markdown link to GitHub's own permalink format — the same URL its "Copy permalink" feature produces,
at the exact commit the line was verified against:

```
[`<file>:<line>`](https://github.com/<owner>/<repo>/blob/<headRefOid>/<path>#L<line>)
```

e.g. `` [`sentry-error-extractor.ts:36`](https://github.com/cioplenu/frontend/blob/c634821.../projects/library/common/services/sentry/sentry-error-extractor.ts#L36) ``.

Never a bare backtick-only citation, never the plain PR URL, never a path with no line.

## Voice

Before writing anything, load the `my-voice` skill and write every comment in that casual register:
short, direct or a question depending on how debatable the point is, backticks around file/function/
variable names, no headings or bullet lists inside a comment, no filler, no "it seems like" AI-isms.
Collapse every layer of the review into ONE flat list of comments — the reader doesn't care which
layer found it, only where and what. Drop anything not worth a teammate's time; a short, high-signal
list beats an exhaustive one.

## Report structure

Write to `pr-reviews/<repo-short-name>-<number>.md` (overwrite if it already exists — a
re-review's old report is stale). Keep the whole thing short, a flat structure, no sections:

```markdown
# <repo-short-name>#<number> — <title>
<url> · ticket: <DEV-XXXX or "none found", one line, flag here if scope doesn't match the PR>

<one-line approval-readiness call: "can't auto-approve, adds a package.json dep" / "auto-approvable,
pure refactor" / etc — name the rule in a few words, nothing longer>

[`<file>:<line>`](<permalink>) — <one to three sentence comment in my-voice casual register. Use a
GitHub-style `suggestion` fenced block instead of prose when there's an exact replacement to give.>

[`<file>:<line>`](<permalink>) — <next comment, same style>

<repeat for every real finding — omit this entire section if there's nothing worth flagging>

<closing line in the exact style of a my-voice review summary — "LGTM 👍🏽" / "Nice, just a couple
comments." / "Main blocker is the <file> issue above, rest is fine." Keep it to one or two sentences,
matching the my-voice approve/request-changes/conditional-approve patterns.>
```

No separate "Strengths" / "What Looks Good" section — if something's worth praising, say it in the
closing line, not its own heading. No "Open questions" heading either — a question is just a comment
anchored to a line, same as a bug.

## Outcome field (read by the tracker app)

Whatever downstream step records `status.json`'s `outcome` field must derive it from the closing line
you just wrote: a request-changes-flavored close ("main blocker is...") → `request-changes`; a clean
approve-flavored close ("LGTM", "nice") → `approve`; anything hedged/conditional or genuinely unclear
→ `needs-discussion`. This is the only structured field a downstream tool reads — get the enum value
exactly right, never invent a fourth value.

## Structured sidecar (for the tracker app's Publish feature)

Alongside the `.md` report, write a second file with the same name but a `.json` extension (e.g.
`frontend-2000.md` → `frontend-2000.json`, same dir, overwrite if present). This is what the tracker
app's "Publish…" button reads to draft a real GitHub review — it is never read by a human, so it holds
the same comments as the `.md` report but as structured data instead of prose-with-links:

```json
{
  "repo": "<owner>/<repo>",
  "number": <PR number, integer>,
  "headRefOid": "<the commit SHA every line above was verified against>",
  "outcome": "approve" | "request-changes" | "needs-discussion",
  "summaryLine": "<the one-line approval-readiness call from the top of the .md report>",
  "closingLine": "<the exact closing line from the bottom of the .md report>",
  "comments": [
    {
      "path": "<same repo-relative path used in the .md citation>",
      "line": <the verified line number, integer, no quotes>,
      "inline": true | false,
      "body": "<the comment text, same wording as the .md report, no markdown link wrapper>",
      "permalink": "<the same blob permalink used in the .md report>"
    }
  ]
}
```

Omit `comments` (empty array) if the report has no findings section. `line` must be the same
grep-verified number used in the `.md` citation — never recompute it separately.

**Setting `inline`**: a GitHub review comment can only attach to a line that actually appears in the
PR's diff (inside one of the `@@ -a,b +c,d @@` hunks you already fetched via `gh pr diff`), not just
anywhere in the file. For each comment, check whether its verified line falls inside the new-file range
(`c` to `c+d-1`) of some hunk for that path in the diff you already have — if it does, `inline: true`;
if it doesn't (e.g. you're commenting on nearby pre-existing code the diff didn't touch), `inline:
false`. When genuinely unsure, default to `false` — a comment that's wrongly marked inline fails to
post as a real GitHub review comment, while one wrongly marked general just posts as regular text in
the review body instead, which is harmless.
