<?php
// tez: loaded before every PHP script through auto_prepend_file. Kept tiny: it runs on every request.
// Works on PHP 7.4 and later.

if (PHP_SAPI === 'cli' && PHP_OS_FAMILY === 'Windows' && !function_exists('pcntl_signal')) {
    $tezCmd = isset($_SERVER['argv'][1]) ? (string) $_SERVER['argv'][1] : '';
    if (strncmp($tezCmd, 'horizon', 7) === 0) {
        require __DIR__ . DIRECTORY_SEPARATOR . 'horizon-windows.php';
    }
    unset($tezCmd);
}

if (!function_exists('dump') && is_file((getenv('TEZ_HOME') ?: getenv('USERPROFILE') . '\\.tez') . '\\run\\dumps-on')) {
    require __DIR__ . DIRECTORY_SEPARATOR . 'dumps.php';
}
