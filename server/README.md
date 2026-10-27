# Tilepicky bucket

A small PHP service that keeps the sheet images of a batch job.
Tilepicky uploads a PNG before it submits an OpenRouter batch, and deletes it when the batch ends.
OpenRouter batch takes public image URLs only, and this service is that URL.
There is no database, no S3, and no account: one token, one directory of images, three PHP files.

## Install

PHP 8.1 or later. No extension is needed beyond the core.

1. Copy this directory to your web space, such as `/www/htdocs/<account>/bucket`.
   It is then public at `https://<host>/bucket`.
2. Copy `config.sample.php` to `config.php` beside it, and fill in `base_url` and `token`.
3. Make `files/` writable by the PHP user, such as `chmod 775 files`.
4. Open `https://<host>/bucket/version.php`. It answers `"configured": true`.
5. In tilepicky, open Settings, AI, your OpenRouter provider.
   Turn on "Upload sheets so a batch can fetch them", fill in the URL and the token, and press "Test connection".

## The API

| Call | Body | Answer |
|---|---|---|
| `GET version.php` | none | `{"bucket":"tilepicky","version":1,"max_bytes":8388608,"configured":true}` |
| `POST upload.php` | the image bytes | `{"key":"...","url":"https://.../files/...","bytes":628}` |
| `POST delete.php` | `{"key":"..."}` | `{"key":"...","deleted":true}` |

Every upload and delete carries the token in an `X-Bucket-Token` header.
A key that is already gone still answers `deleted: true`, so a retry after a lost answer is safe.

## What it keeps

Images only: PNG, JPEG, WebP, and GIF, up to 8 MB, under a random 32-hex name.
Tilepicky uploads PNG, and deletes each image when its batch ends.
A crash or a cancelled job can leave images behind. A cron job can clear the old ones:

```
find /path/to/bucket/files -type f -mtime +2 -delete
```

## The token

The token is the only credential. Anyone who has it can upload and delete.
`config.php` holds it, and `version.php` never reports it.
The `.htaccess` files deny `config.php` and `bucket.php` over the web, and stop a script that reaches `files/` from running.

The token travels in `X-Bucket-Token`, not in `Authorization: Bearer`.
Apache hands the `Authorization` header to PHP only when a rewrite rule copies it, and `X-Bucket-Token` needs no such rule.
nginx and the PHP built-in server pass both.

## Try it on this machine first

```
cp config.sample.php config.php    # then edit it: base_url http://127.0.0.1:8099, any token
php -S 127.0.0.1:8099 -t .
curl -X POST -H 'X-Bucket-Token: <token>' --data-binary @../icon.png http://127.0.0.1:8099/upload.php
```

The built-in server ignores `.htaccess`, so test the denials on the host.
