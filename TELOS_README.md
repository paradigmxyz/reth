# telos-reth

## Rebase notes:
```bash
git checkout main
git fetch upstream
git rebase <SHA OF UPSTREAM RELEASE TAG>
git push
git checkout telos-main
git rebase main # this is where it might get tricky! :)
```
