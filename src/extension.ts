// The module 'vscode' contains the VS Code extensibility API
// Import the module and reference it with the alias vscode in your code below
import * as vscode from 'vscode'
import * as cproc from 'node:child_process'
import * as net from 'net'
import pkg from '../package.json'

const ID_PIPE_SERVER = pkg['named-pipe']

const ID_PROTO_GET_STATUS = 71
const ID_PROTO_NOTIFY_STOP = 72

const ID_PROTO_SAY_OK = 171
const ID_PROTO_GRIPE_REGEX = 172

const PIPE_CLIENT = new (class PipeClient {
    client?: net.Socket
    constructor() {}
    connect(output?: vscode.OutputChannel) {
        if (this.client !== undefined) {
            this.disconnect()
            output?.appendLine(
                `** Dropping an existing connection to the CtrlOEM3 service. `
            )
        }

        let client = net.connect(ID_PIPE_SERVER)
        this.client = client

        client.on('connect', () => {
            client?.write(Uint8Array.from([ID_PROTO_GET_STATUS]))
        })
        client.on('data', (data) => {
            let router: { [key: number]: () => void } = {
                [ID_PROTO_GRIPE_REGEX]: () => {
                    vscode.window
                        .showErrorMessage(
                            'Failed to compile `ctrl-oem3.matches-window-title`, check output console for details. ',
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
                                        'ctrl-oem3.stop'
                                    )
                                    setTimeout(() => {
                                        vscode.commands.executeCommand(
                                            'ctrl-oem3.start'
                                        )
                                    }, 500)
                                    break
                            }
                        })
                },
                [ID_PROTO_SAY_OK]: () => {
                    output?.appendLine(`** Connected to the CtrlOEM3 service. `)
                },
            }
            for (let cmd of data) {
                if (router[cmd] === undefined) {
                    output?.appendLine(
                        `** Received an unknown ACK=${cmd} from the CtrlOEM3 service. `
                    )
                } else {
                    router[cmd]()
                }
            }
        })
        client.on('error', (err) => {
            output?.appendLine(
                `** Failed to connect to the CtrlOEM3 service: ${err}`
            )
            vscode.window
                .showErrorMessage(
                    'Failed to connect to the CtrlOEM3 service, check the output console for details. ',
                    'Show Output',
                    'Restart Extension'
                )
                .then((sel) => {
                    switch (sel) {
                        case 'Restart Extension':
                            vscode.commands.executeCommand('ctrl-oem3.start')
                            break
                        case 'Show Output':
                            output?.show()
                            break
                    }
                })
        })
        client.on('close', () => {
            if (Object.is(this.client, client)) {
                output?.appendLine(`** Disconnected to the CtrlOEM3 service. `)
                this.disconnect()
            } else {
                output?.appendLine(
                    `** An outdated connection to the CtrlOEM3 service dropped as notified. `
                )
            }
        })
    }
    disconnect() {
        this.client?.end()
        this.client = undefined
    }
})()

// This method is called when your extension is activated
// Your extension is activated the very first time the command is executed
export function activate(context: vscode.ExtensionContext) {
    let native_path = context.asAbsolutePath('dist/ctrl-oem3-native.exe')
    let output = vscode.window.createOutputChannel('CtrlOEM3')

    let start = vscode.commands.registerCommand('ctrl-oem3.start', () => {
        command_start(native_path, output)
    })
    let stop = vscode.commands.registerCommand('ctrl-oem3.stop', () => {
        command_stop(output)
    })

    context.subscriptions.push(start, stop, output)

    if (
        vscode.workspace.getConfiguration('ctrl-oem3').get<boolean>('autostart')
    ) {
        vscode.commands.executeCommand('ctrl-oem3.start')
    }
}

// This method is called when your extension is deactivated
export function deactivate() {
    PIPE_CLIENT.disconnect()
}

function command_start(native_path: string, output: vscode.OutputChannel) {
    output?.appendLine(`** Starting the CtrlOEM3 service. `)

    let pattern = vscode.workspace
        .getConfiguration('ctrl-oem3')
        .get<string>('matches-window-title')
    let encoded_pattern = Buffer.from(
        new TextEncoder().encode(pattern)
    ).toString('base64')

    let hproc = cproc.spawn(
        native_path,
        [`--matches-window-title=${encoded_pattern}`],
        {
            detached: true,
        }
    )
    if (hproc.pid === undefined) {
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
                        vscode.commands.executeCommand('ctrl-oem3.start')
                        break
                }
            })
    }

    hproc.stdout.on('data', (data: any) => output.append(data.toString()))
    hproc.stderr.on('data', (data: any) => output.append(data.toString()))
    hproc.on('exit', (code) => {
        if (code === 0) {
            output.appendLine(
                `** Current window's CtrlOEM3 instance exited as notified. `
            )
        } else {
            output.appendLine(
                `** Current window's CtrlOEM3 instance crashed, ExitCode=${code}. `
            )
            vscode.window
                .showErrorMessage(
                    'CtrlOEM3 crashed, check the output console for details. ',
                    'Show Output',
                    'Restart Extension'
                )
                .then((sel) => {
                    switch (sel) {
                        case 'Show Output':
                            output.show()
                            break
                        case 'Restart Extension':
                            vscode.commands.executeCommand('ctrl-oem3.start')
                            break
                    }
                })
        }
    })

    setTimeout(() => {
        PIPE_CLIENT.connect(output)
    }, 400)
}

function command_stop(output?: vscode.OutputChannel) {
    output?.appendLine('** Stopping the CtrlOEM3 service. ')

    let client = net.connect(ID_PIPE_SERVER)

    client.on('connect', () => {
        client.write(Uint8Array.from([ID_PROTO_NOTIFY_STOP]))
    })
    client.on('error', (err) => {
        output?.appendLine(`** Failed to stop the CtrlOEM3 service: ${err}`)
    })
}
