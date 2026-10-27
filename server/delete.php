<?php
// SPDX-License-Identifier: GPL-3.0-only
// POST {"key": "..."} with the bucket token. A key that is already gone is
// not an error, so a retry after a lost answer is safe.

require __DIR__ . '/bucket.php';

$config = bucket_config();
bucket_authorize($config);
if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') {
    bucket_reply(405, ['error' => 'Use POST with {"key": "..."} as the body.']);
}

$sent = json_decode((string) file_get_contents('php://input'), true);
$key = is_array($sent) && is_string($sent['key'] ?? null) ? $sent['key'] : '';
if (!bucket_key_is_safe($key)) {
    bucket_reply(400, ['error' => 'The key is not one the bucket issues.']);
}

$path = bucket_data_dir($config) . '/' . $key;
if (is_file($path) && !@unlink($path)) {
    bucket_reply(500, ['error' => 'The bucket could not delete the image.']);
}

bucket_reply(200, ['key' => $key, 'deleted' => true]);
