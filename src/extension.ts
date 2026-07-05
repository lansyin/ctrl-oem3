import * as vscode from 'vscode'
import * as cproc from 'node:child_process'
import * as net from 'net'

const PIPE_SERVER = String.raw`\\.\pipe\vscode.ext.ctrl-oem3`
const REQUEST_SHUTDOWN = 72

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
}

class PipeClient {
    private _process?: cproc.ChildProcess
    private _socket?: net.Socket
    private _should_stop = false
    private _started = false
    private _on_disconnected?: () => void
    private _status_bar: vscode.StatusBarItem

    constructor(status_bar: vscode.StatusBarItem) {
        this._status_bar = status_bar
    }

    get started() {
        return this._started
    }

    async connect(nativePath: string, output: vscode.OutputChannel) {
        this.disconnect()
        this._should_stop = false
        this._started = true

        for (let attempt = 1; attempt <= 3; attempt++) {
            if (this._should_stop) break
            let tag = attempt > 1 ? ` (${attempt}/3)` : ''
            output.appendLine(`** Starting the CtrlOEM3 service.${tag}`)
            this._set_error(`Connecting${tag}...`)

            let pattern = vscode.workspace
                .getConfiguration('ctrl-oem3')
                .get<string>('matches-window-title')
            let encoded = Buffer.from(
                new TextEncoder().encode(pattern)
            ).toString('base64')

            const proc = cproc.spawn(
                nativePath,
                [`--matches-window-title=${encoded}`],
                { detached: true }
            )
            this._process = proc

            if (proc.pid === undefined) {
                output.appendLine('** Failed to spawn native process. ')
                this._set_error(`Spawn failed${tag}`)
                vscode.window
                    .showErrorMessage(
                        'Failed to start a CtrlOEM3 instance, check the output console for details. ',
                        'Show Output',
                        'Restart Extension'
                    )
                    .then((sel) => {
                        switch (sel) {
                            case 'Show Output':
                                output.show()
                                break
                            case 'Restart Extension':
                                vscode.commands.executeCommand('ctrl-oem3.stop')
                                setTimeout(
                                    () =>
                                        vscode.commands.executeCommand(
                                            'ctrl-oem3.start'
                                        ),
                                    500
                                )
                                break
                        }
                    })
            }

            proc.stdout?.on('data', (data: any) =>
                output.append(data.toString())
            )
            proc.stderr?.on('data', (data: any) => {
                const text = data.toString()
                output.append(text)
                const fatal_prefix = 'vscode.ext.ctrl-oem3.fatal:'
                if (text.includes(fatal_prefix)) {
                    const msg_start =
                        text.indexOf(fatal_prefix) + fatal_prefix.length
                    const detail = text.substring(msg_start).trim()
                    this._set_error(`Fatal error${tag}: ${detail}`)
                    vscode.window
                        .showErrorMessage(
                            `Fatal error: ${detail}`,
                            'Show Output',
                            'Edit Settings',
                            'Restart Extension'
                        )
                        .then((sel) => {
                            switch (sel) {
                                case 'Show Output':
                                    output?.show()
                                    break
                                case 'Edit Settings':
                                    vscode.commands.executeCommand(
                                        'workbench.action.openSettings',
                                        '@ext:ctrl-oem3.matches-window-title'
                                    )
                                    break
                                case 'Restart Extension':
                                    vscode.commands.executeCommand(
                                        'ctrl-oem3.restart'
                                    )
                                    break
                            }
                        })
                }
            })
            proc.on('exit', (code) => {
                output.appendLine(
                    `** Current window's CtrlOEM3 instance exited, ExitCode=${code}. `
                )
            })

            await sleep(500)
            if (this._should_stop) break

            try {
                await this._try_connect(output)
            } catch (err: any) {
                output.appendLine(`** Connection failed${tag}: ${err.message}`)
                this._set_error(`Connection failed${tag}`)
                this._kill_process()
                if (attempt < 3) {
                    await sleep(15000)
                }
                continue
            }

            this._set_connected()
            this._process = undefined
            await new Promise<void>((resolve) => {
                this._on_disconnected = resolve
                if (this._socket === undefined) resolve()
            })
            this._on_disconnected = undefined
            this._set_error('CtrlOEM3 service disconnected, click to restart')
            return
        }

        this._set_error('Click to restart CtrlOEM3')
        this._status_bar.command = 'ctrl-oem3.start'
        output.appendLine('** All 3 connection attempts failed, giving up. ')
    }

    disconnect() {
        this._should_stop = true
        this._started = false
        this._socket?.end()
        this._socket = undefined
        this._kill_process()
        this._on_disconnected?.()
    }

    private _set_connected() {
        this._status_bar.text = 'CtrlOEM3'
        this._status_bar.tooltip = 'CtrlOEM3 active: click to view output'
        this._status_bar.command = 'ctrl-oem3.show-output'
        this._status_bar.backgroundColor = undefined
        this._status_bar.show()
    }

    private _set_error(tooltip: string) {
        this._status_bar.text = '$(warning) CtrlOEM3'
        this._status_bar.tooltip = tooltip
        this._status_bar.command = 'ctrl-oem3.restart'
        this._status_bar.backgroundColor = new vscode.ThemeColor(
            'statusBarItem.warningBackground'
        )
        this._status_bar.show()
    }

    private _try_connect(output: vscode.OutputChannel): Promise<void> {
        return new Promise((resolve, reject) => {
            const socket = net.connect(PIPE_SERVER)
            this._socket = socket

            socket.on('close', () => {
                this._socket = undefined
                this._on_disconnected?.()
            })

            const timer = setTimeout(() => {
                socket.destroy()
                reject(new Error('connection timeout'))
            }, 3000)

            socket.on('connect', () => {
                clearTimeout(timer)
                output.appendLine('** Connected to the CtrlOEM3 service. ')
                resolve()
            })

            socket.on('error', (err) => {
                clearTimeout(timer)
                reject(err)
            })
        })
    }

    private _kill_process() {
        if (this._process) {
            this._process.kill()
            this._process = undefined
        }
    }
}

let PIPE_CLIENT: PipeClient | undefined

export function activate(context: vscode.ExtensionContext) {
    let native_path = context.asAbsolutePath('dist/ctrl-oem3-native.exe')
    let output = vscode.window.createOutputChannel('CtrlOEM3')

    let status_bar = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        0
    )
    status_bar.text = '$(circuit-board) CtrlOEM3'
    status_bar.tooltip = 'CtrlOEM3: click to start'
    status_bar.command = 'ctrl-oem3.start'
    status_bar.show()

    PIPE_CLIENT = new PipeClient(status_bar)

    let show_output = vscode.commands.registerCommand(
        'ctrl-oem3.show-output',
        () => output.show()
    )
    let restart = vscode.commands.registerCommand('ctrl-oem3.restart', () => {
        vscode.commands.executeCommand('ctrl-oem3.stop')
        setTimeout(() => vscode.commands.executeCommand('ctrl-oem3.start'), 500)
    })
    let start = vscode.commands.registerCommand('ctrl-oem3.start', () =>
        command_start(native_path, output, PIPE_CLIENT!)
    )
    let stop = vscode.commands.registerCommand('ctrl-oem3.stop', () =>
        command_stop(output, PIPE_CLIENT!)
    )

    let config_change = vscode.workspace.onDidChangeConfiguration((e) => {
        if (
            e.affectsConfiguration('ctrl-oem3.matches-window-title') &&
            PIPE_CLIENT?.started
        ) {
            output.appendLine(
                '** Configuration changed, restarting the CtrlOEM3 service. '
            )
            vscode.commands.executeCommand('ctrl-oem3.restart')
        }
    })

    context.subscriptions.push(
        show_output,
        restart,
        start,
        stop,
        config_change,
        output,
        status_bar
    )

    if (
        vscode.workspace.getConfiguration('ctrl-oem3').get<boolean>('autostart')
    ) {
        vscode.commands.executeCommand('ctrl-oem3.start')
    }
}

export function deactivate() {
    PIPE_CLIENT?.disconnect()
}

async function command_start(
    native_path: string,
    output: vscode.OutputChannel,
    client: PipeClient
) {
    await client.connect(native_path, output)
}

async function command_stop(
    output?: vscode.OutputChannel,
    client?: PipeClient
) {
    output?.appendLine('** Stopping the CtrlOEM3 service. ')
    client?.disconnect()

    let conn = net.connect(PIPE_SERVER)
    conn.on('connect', () => {
        conn.end(Uint8Array.from([REQUEST_SHUTDOWN]))
    })
    conn.on('error', () => {})
}
