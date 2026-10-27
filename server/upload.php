<?php
// SPDX-License-Identifier: GPL-3.0-only
// POST one image as the body. The answer carries the key and the public URL.

require __DIR__ . '/bucket.php';

$config = bucket_config();
bucket_authorize($config);
if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') {
    bucket_reply(405, ['error' => 'Use POST with the image as the body.']);
}

$body = file_get_contents('php://input');
$size = is_string($body) ? strlen($body) : 0;
if ($size === 0) {
    bucket_reply(400, ['error' => 'The request has no image.']);
}
if ($size > BUCKET_MAX_BYTES) {
    bucket_reply(413, ['error' => 'The image is larger than ' . (BUCKET_MAX_BYTES >> 20) . ' MB.']);
}

$image = @getimagesizefromstring($body);
$extension = $image === false ? null : (BUCKET_EXTENSIONS[$image[2]] ?? null);
if ($extension === null) {
    bucket_reply(415, ['error' => 'Send a PNG, JPEG, WebP, or GIF image.']);
}

$key = bin2hex(random_bytes(16)) . '.' . $extension;
$path = bucket_data_dir($config) . '/' . $key;
if (@file_put_contents($path, $body, LOCK_EX) !== $size) {
    @unlink($path);
    bucket_reply(500, ['error' => 'The bucket could not write the image.']);
}
@chmod($path, 0644);

bucket_reply(200, ['key' => $key, 'url' => rtrim($config['base_url'], '/') . '/files/' . $key, 'bytes' => $size]);
