<?php
// SPDX-License-Identifier: GPL-3.0-only
// Copy this file to config.php beside it, then fill in both values.
// config.php holds a secret. Keep it out of git, and out of any backup you share.

return [
    // The public address of this directory, without a trailing slash.
    // Every image URL the bucket answers with is built from it.
    'base_url' => 'https://tilepicky.experimentehrlichkeit.de/bucket',

    // The token tilepicky sends. Generate one with:
    //   php -r 'echo bin2hex(random_bytes(32)), PHP_EOL;'
    'token' => 'put-a-long-random-token-here',

    // Where the images go. Leave this line out to use `files/` beside this file.
    // 'data_dir' => __DIR__ . '/files',
];
