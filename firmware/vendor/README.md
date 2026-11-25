# Stock firmware images (not in this repository)

`tools/rollback.sh` and `tools/flash_uf2.py` expect the vendor's stock image here as
`firmware/vendor/0.0.18.uf2` (and optionally the older `0.0.9.uf2`). These are HOVEY WORKS'
property and are not redistributed.

Get them from the official web driver, https://hoveyworks-driver.com/ — its firmware page offers
`0.0.18.uf2` ("latest") and `0.0.9.uf2` ("stable"). Put the file in this directory under the
same name. `tools/tests/test_flash_guard.py` checks them when present and skips otherwise.
