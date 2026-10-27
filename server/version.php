<?php
// SPDX-License-Identifier: GPL-3.0-only
// Public. What the bucket is, without the token and without the config.
// Tilepicky shows this answer in its settings.

require __DIR__ . '/bucket.php';

bucket_reply(200, [
    'bucket' => 'tilepicky',
    'version' => 1,
    'max_bytes' => BUCKET_MAX_BYTES,
    'configured' => is_file(__DIR__ . '/config.php'),
]);
