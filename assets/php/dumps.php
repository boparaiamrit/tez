<?php
// tez Dumps: every dump() and dd() is also sent to the tez dashboard (Dumps tab), with where it
// came from. The page still shows it as usual. Loaded only while Dumps is turned on.

if (!function_exists('tez_send_dump')) {
    function tez_send_dump($var)
    {
        $html = null;
        if (class_exists('Symfony\Component\VarDumper\Cloner\VarCloner') && class_exists('Symfony\Component\VarDumper\Dumper\HtmlDumper')) {
            try {
                $cloner = new \Symfony\Component\VarDumper\Cloner\VarCloner();
                $dumper = new \Symfony\Component\VarDumper\Dumper\HtmlDumper();
                $html = $dumper->dump($cloner->cloneVar($var), true);
            } catch (\Throwable $e) {
                $html = null;
            }
        }
        if ($html === null) {
            ob_start();
            var_dump($var);
            $html = '<pre>' . htmlspecialchars((string) ob_get_clean()) . '</pre>';
        }
        $trace = debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS, 4);
        $at = ['file' => '', 'line' => 0];
        foreach ($trace as $frame) {
            if (isset($frame['file']) && strpos($frame['file'], 'dumps.php') === false) {
                $at = ['file' => $frame['file'], 'line' => isset($frame['line']) ? $frame['line'] : 0];
                break;
            }
        }
        $payload = json_encode([
            'time' => microtime(true),
            'site' => getenv('TEZ_SITE') ?: (isset($_SERVER['HTTP_HOST']) ? $_SERVER['HTTP_HOST'] : 'cli'),
            'uri' => isset($_SERVER['REQUEST_URI']) ? $_SERVER['REQUEST_URI'] : implode(' ', isset($_SERVER['argv']) ? $_SERVER['argv'] : []),
            'file' => $at['file'],
            'line' => $at['line'],
            'html' => $html,
        ]);
        $sock = @stream_socket_client('tcp://127.0.0.1:' . (getenv('TEZ_API_PORT') ?: '7979'), $errno, $errstr, 0.25);
        if ($sock) {
            stream_set_timeout($sock, 0, 250000);
            fwrite($sock, "POST /api/dumps HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Tez: dump\r\nContent-Type: application/json\r\nContent-Length: " . strlen($payload) . "\r\nConnection: close\r\n\r\n" . $payload);
            fread($sock, 64);
            fclose($sock);
        }
    }

    function dump($var, ...$moreVars)
    {
        foreach (array_merge([$var], $moreVars) as $v) {
            tez_send_dump($v);
            if (class_exists('Symfony\Component\VarDumper\VarDumper')) {
                \Symfony\Component\VarDumper\VarDumper::dump($v);
            } else {
                var_dump($v);
            }
        }
        return $moreVars ? array_merge([$var], $moreVars) : $var;
    }

    function dd(...$vars)
    {
        if (!in_array(PHP_SAPI, ['cli', 'phpdbg'], true) && !headers_sent()) {
            header('HTTP/1.1 500 Internal Server Error');
        }
        foreach ($vars as $v) {
            tez_send_dump($v);
            if (class_exists('Symfony\Component\VarDumper\VarDumper')) {
                \Symfony\Component\VarDumper\VarDumper::dump($v);
            } else {
                var_dump($v);
            }
        }
        exit(1);
    }
}
