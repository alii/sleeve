# Contributing to Sleeve

Thanks for wanting to help. Sleeve is a small project run by a few people, so these rules
are here to keep it easy to look after.

## New here? Open an issue first

If you don't have a commit in Sleeve's git history yet, please don't open a pull request
straight away. Open an issue first and say what you'd like to change. Wait for a yes, then
send the PR. A PR from a new contributor with no issue behind it may be closed without a
review.

You can check whether you count as a contributor already:

```sh
git log --author="you@example.com" --oneline | head -1
```

If that prints a commit, you're a contributor and the rules in this section don't apply to
you.

## Write your own issues and PRs

If you're new, write your issue and PR descriptions yourself, in your own words. Don't
paste in text from an AI tool. Using AI to help write code is fine, but you need to
understand every line you send and be able to answer questions about it.

Short and plain is great. Tell us:

- what's wrong or what you want
- how to see it happen, for a bug
- what you tried, and what you're not sure about

## Making changes

1. Install Rust with [rustup](https://rustup.rs).
2. Run it with `cargo run`, and run the tests with `cargo test`.
3. Keep each PR to one change. Small PRs get merged faster.
4. Never test against your real rekordbox library. Use a copy, because a bug can damage it.

## Commit messages

Keep them short and simple. One line of up to about ten words is plenty, like
`fix the deck playing after eject`.
