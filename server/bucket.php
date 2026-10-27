<?php
// SPDX-License-Identifier: GPL-3.0-only
// What the three entry files share: the config, the token check, the replies,
// and the rule for a key the bucket issues.

declare(strict_types=1);

// The largest image the bucket takes.
const BUCKET_MAX_BYTES = 8 * 1024 * 1024;

// The image types the bucket stores, by the type number `getimagesize` reports.
const BUCKET_EXTENSIONS = [IMAGETYPE_PNG => 'png', IMAGETYPE_JPEG => 'jpg', IMAGETYPE_WEBP => 'webp', IMAGETYPE_GIF => 'gif'];

// One JSON answer and the end of the request.
function bucket_reply(int $status, array $body): never
{
    http_response_code($status);
    header('Content-Type: application/json');
    header('Cache-Control: no-store');
    echo json_encode($body, JSON_UNESCAPED_SLASHES), "\n";
    exit;
}

// The settings from config.php. It answers 500 when the file is missing or wrong.
function bucket_config(): array
{
    static $config = null;
    if (is_array($config)) {
        return $config;
    }
    $path = __DIR__ . '/config.php';
    if (!is_file($path)) {
        bucket_reply(500, ['error' => 'The bucket has no config.php. Copy config.sample.php to config.php.']);
    }
    $saved = require $path;
    if (!is_array($saved)) {
        bucket_reply(500, ['error' => 'config.php must return an array.']);
    }
    foreach (['token', 'base_url'] as $name) {
        if (!is_string($saved[$name] ?? null) || trim($saved[$name]) === '') {
            bucket_reply(500, ['error' => "config.php needs a non-empty $name."]);
        }
    }
    $config = $saved + ['data_dir' => __DIR__ . '/files'];
    return $config;
}

// The one credential. A wrong or missing token stops the request here.
function bucket_authorize(array $config): void
{
    // Not `Authorization`: Apache hands that header to PHP only with extra
    // rewrite rules, and a header of our own needs none.
    $sent = $_SERVER['HTTP_X_BUCKET_TOKEN'] ?? '';
    if (!is_string($sent) || $sent === '') {
        bucket_reply(401, ['error' => 'Send the bucket token in the X-Bucket-Token header.']);
    }
    if (!hash_equals($config['token'], $sent)) {
        bucket_reply(401, ['error' => 'The bucket token is wrong.']);
    }
}

// The directory the images go in, made when it is missing.
function bucket_data_dir(array $config): string
{
    $dir = rtrim($config['data_dir'], '/');
    if (!is_dir($dir) && !@mkdir($dir, 0775, true) && !is_dir($dir)) {
        bucket_reply(500, ['error' => 'The bucket cannot make its data directory.']);
    }
    return $dir;
}

// A key the bucket issued, and nothing else: no path, no other extension.
function bucket_key_is_safe(string $key): bool
{
    return preg_match('/\A[a-f0-9]{32}\.(?:png|jpg|webp|gif)\z/', $key) === 1;
}
