<?php
// tez: lets Laravel Horizon run natively on Windows.
//
// Windows PHP has no pcntl or posix extension, which Horizon uses to signal its master, supervisor
// and worker processes. This file is loaded only into `artisan horizon*` processes and provides:
//   * the SIG* constants and the pcntl_* / posix_* functions Horizon calls;
//   * signal delivery between processes through small mailbox files, picked up every second by the
//     master and supervisors (through their sleep(1) loop), so `horizon:terminate`, `horizon:pause`
//     and `horizon:continue` reach their handlers;
//   * process liveness through kernel32 (FFI), with a tasklist fallback.
// Workers themselves are stopped by Horizon through Symfony Process, which uses taskkill on Windows.
// Everything Horizon writes to Redis (the dashboard's data) is Horizon's own code, unchanged.

namespace {
    foreach ([
        'SIGHUP' => 1, 'SIGINT' => 2, 'SIGQUIT' => 3, 'SIGILL' => 4, 'SIGTRAP' => 5, 'SIGABRT' => 6,
        'SIGBUS' => 7, 'SIGFPE' => 8, 'SIGKILL' => 9, 'SIGUSR1' => 10, 'SIGSEGV' => 11, 'SIGUSR2' => 12,
        'SIGPIPE' => 13, 'SIGALRM' => 14, 'SIGTERM' => 15, 'SIGCHLD' => 17, 'SIGCONT' => 18,
        'SIGSTOP' => 19, 'SIGTSTP' => 20, 'SIGTTIN' => 21, 'SIGTTOU' => 22, 'SIGWINCH' => 28,
        'SIG_DFL' => 0, 'SIG_IGN' => 1, 'SIG_ERR' => -1, 'WNOHANG' => 1, 'WUNTRACED' => 2,
        'SIG_BLOCK' => 0, 'SIG_UNBLOCK' => 1, 'SIG_SETMASK' => 2,
    ] as $tezName => $tezValue) {
        if (!defined($tezName)) {
            define($tezName, $tezValue);
        }
    }
    unset($tezName, $tezValue);

    final class TezSignals
    {
        /** @var array<int, callable|int> */
        public static $handlers = [];
        public static $async = false;
        private static $ffi = null;
        private static $dir = null;

        public static function dir()
        {
            if (self::$dir === null) {
                $home = getenv('TEZ_HOME') ?: (getenv('USERPROFILE') . '\\.tez');
                self::$dir = $home . '\\run\\signals';
                if (!is_dir(self::$dir)) {
                    @mkdir(self::$dir, 0777, true);
                }
            }
            return self::$dir;
        }

        public static function alive($pid)
        {
            $pid = (int) $pid;
            if ($pid <= 0) {
                return false;
            }
            if ($pid === getmypid()) {
                return true;
            }
            if (self::$ffi === null) {
                self::$ffi = false;
                if (extension_loaded('ffi')) {
                    try {
                        self::$ffi = \FFI::cdef(
                            'void* OpenProcess(unsigned long access, int inherit, unsigned long pid);'
                            . 'int GetExitCodeProcess(void* process, unsigned long* code);'
                            . 'int CloseHandle(void* handle);',
                            'kernel32.dll'
                        );
                    } catch (\Throwable $e) {
                        self::$ffi = false;
                    }
                }
            }
            if (self::$ffi) {
                $h = self::$ffi->OpenProcess(0x1000, 0, $pid);
                if ($h === null || \FFI::isNull($h)) {
                    return false;
                }
                $code = self::$ffi->new('unsigned long');
                $ok = self::$ffi->GetExitCodeProcess($h, \FFI::addr($code));
                self::$ffi->CloseHandle($h);
                return $ok && $code->cdata == 259;
            }
            $out = (string) @shell_exec('tasklist /FI "PID eq ' . $pid . '" /NH /FO CSV 2>NUL');
            return strpos($out, '"' . $pid . '"') !== false;
        }

        public static function kill($pid)
        {
            @exec('taskkill /F /T /PID ' . (int) $pid . ' 2>NUL', $out, $code);
            return $code === 0;
        }

        public static function send($pid, $signal)
        {
            $pid = (int) $pid;
            $signal = (int) $signal;
            if ($signal === 0) {
                return self::alive($pid);
            }
            if (!self::alive($pid)) {
                return false;
            }
            if ($signal === SIGKILL) {
                return self::kill($pid);
            }
            if ($pid === getmypid()) {
                self::deliver($signal);
                return true;
            }
            return @file_put_contents(self::dir() . '\\' . $pid, $signal . "\n", FILE_APPEND | LOCK_EX) !== false;
        }

        public static function deliver($signal)
        {
            $handler = isset(self::$handlers[$signal]) ? self::$handlers[$signal] : SIG_DFL;
            if (is_callable($handler)) {
                $handler($signal, []);
            } elseif ($handler === SIG_DFL && in_array($signal, [SIGTERM, SIGINT, SIGQUIT, SIGHUP], true)) {
                exit(0);
            }
        }

        public static function dispatch()
        {
            $file = self::dir() . '\\' . getmypid();
            if (!is_file($file)) {
                return;
            }
            $data = (string) @file_get_contents($file);
            @unlink($file);
            foreach (array_filter(array_map('intval', explode("\n", $data))) as $signal) {
                self::deliver($signal);
            }
        }
    }

    if (!function_exists('pcntl_async_signals')) {
        function pcntl_async_signals($enable = null)
        {
            $old = TezSignals::$async;
            if ($enable !== null) {
                TezSignals::$async = (bool) $enable;
            }
            return $old;
        }
        function pcntl_signal($signal, $handler, $restart_syscalls = true)
        {
            TezSignals::$handlers[(int) $signal] = $handler;
            return true;
        }
        function pcntl_signal_get_handler($signal)
        {
            return isset(TezSignals::$handlers[(int) $signal]) ? TezSignals::$handlers[(int) $signal] : SIG_DFL;
        }
        function pcntl_signal_dispatch()
        {
            TezSignals::dispatch();
            return true;
        }
        function pcntl_alarm($seconds)
        {
            return 0;
        }
        function pcntl_sigprocmask($how, $set, &$old_set = null)
        {
            $old_set = [];
            return true;
        }
    }

    if (!function_exists('posix_kill')) {
        function posix_kill($pid, $signal)
        {
            return TezSignals::send($pid, $signal);
        }
        function posix_getpid()
        {
            return getmypid();
        }
        function posix_getppid()
        {
            static $parent = null;
            if ($parent === null) {
                $parent = 0;
                foreach (isset($_SERVER['argv']) ? $_SERVER['argv'] : [] as $arg) {
                    if (strncmp((string) $arg, '--parent-id=', 12) === 0) {
                        $parent = (int) substr($arg, 12);
                    }
                }
            }
            if ($parent > 1) {
                // Horizon exits a supervisor whose master is gone; report 1 ("orphaned") in that case.
                return TezSignals::alive($parent) ? $parent : 1;
            }
            return 2;
        }
        function posix_get_last_error()
        {
            return 0;
        }
        function posix_strerror($errno)
        {
            return 'not available on Windows';
        }
    }
}

namespace Laravel\Horizon {
    // Horizon's master and supervisors loop on sleep(1). Unqualified calls inside this namespace
    // resolve here first, which is where queued signals get delivered.
    if (!function_exists('Laravel\Horizon\sleep')) {
        function sleep($seconds)
        {
            \TezSignals::dispatch();
            $r = \sleep($seconds);
            \TezSignals::dispatch();
            return $r;
        }
    }

    // `ps aux | grep` does not exist on Windows; Horizon only uses this count to subtract workers
    // left over from an earlier run, and tez ends those with the old process tree.
    if (!class_exists('Laravel\Horizon\SystemProcessCounter', false)) {
        class SystemProcessCounter
        {
            public static $command = 'horizon:work';

            public function get($name)
            {
                return 0;
            }
        }
    }
}
