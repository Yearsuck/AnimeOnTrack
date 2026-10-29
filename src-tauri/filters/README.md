# Bundled filter lists

`lists.txt.gz` is the gzip of these public filter lists concatenated, with comment (`!`) and header lines stripped, snapshot of 2026-10-29. They feed the `adblock` crate (Brave's adblock-rust, MPL-2.0) in `src/ad_block.rs` for network filtering inside the app's own viewer windows.

| List | Source | License |
|---|---|---|
| EasyList | https://easylist.to/easylist/easylist.txt | GPLv3+ / CC BY-SA 3.0 (dual) |
| EasyList Spanish | https://easylist-downloads.adblockplus.org/easylistspanish.txt | GPLv3+ / CC BY-SA 3.0 (dual) |
| EasyPrivacy | https://easylist.to/easylist/easyprivacy.txt | GPLv3+ / CC BY-SA 3.0 (dual) |
| uBlock filters | https://github.com/uBlockOrigin/uAssets (`filters/filters.txt`) | GPLv3 |
| uBlock annoyances (others) | https://github.com/uBlockOrigin/uAssets (`filters/annoyances-others.txt`) | GPLv3 |

To refresh: download the files, drop lines starting with `!` and `[Adblock`, drop CRs and empty lines, concatenate in the order above, `gzip -9` to `lists.txt.gz`.
