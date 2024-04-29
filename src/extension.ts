// The module 'vscode' contains the VS Code extensibility API
// Import the module and reference it with the alias vscode in your code below
import * as vscode from 'vscode';
import * as cproc from 'node:child_process';
import * as net from "net";


const ID_PIPE_SERVER = "\\\\.\\pipe\\vscode_extension-ctrl_oem3";

const ID_PROTO_GET_STATUS = 71;
const ID_PROTO_NOTIFY_STOP = 72;

const ID_PROTO_SAY_OK = 171;
const ID_PROTO_GRIPE_REGEX = 172;

// This method is called when your extension is activated
// Your extension is activated the very first time the command is executed
export function activate(context: vscode.ExtensionContext) {
	let native_path = context.asAbsolutePath('dist/ctrl-oem3-native.exe');
	let output = vscode.window.createOutputChannel('CtrlOEM3');

	let start = vscode.commands.registerCommand('ctrl-oem3.start', () => { command_start(native_path, output); });
	let stop = vscode.commands.registerCommand('ctrl-oem3.stop', () => { command_stop(native_path, output); });

	context.subscriptions.push(start, stop, output);

	if (vscode.workspace.getConfiguration('ctrl-oem3').get<boolean>("autostart")) {
		vscode.commands.executeCommand('ctrl-oem3.start');
	}
}

// This method is called when your extension is deactivated
export function deactivate() { }

function command_start(native_path: string, output: vscode.OutputChannel) {
	let pattern = vscode.workspace.getConfiguration('ctrl-oem3').get<string>("matches-window-title");
	let encoded_pattern = Buffer.from(new TextEncoder().encode(pattern)).toString('base64');

	let hproc = cproc.spawn(native_path, [`--matches-window-title=${encoded_pattern}`], {
		detached: true,
	});
	if (hproc.pid === undefined) {
		vscode.window.showErrorMessage('Failed to start CtrlOEM3 service, check output console for details. ', 'Show Output', 'Restart Extension').then((sel) => {
			switch (sel) {
				case 'Show Output':
					output.show();
					break;
				case 'Restart Extension':
					vscode.commands.executeCommand('ctrl-oem3.start');
					break;
			}
		});
	}

	hproc.stdout.on('data', (data: any) => output.append(data.toString()));
	hproc.stderr.on('data', (data: any) => output.append(data.toString()));
	hproc.on('exit', (code) => {
		if (code === 0) {
			output.appendLine(`** Current window's CtrlOEM3 instance is notified to exit. `);
		} else {
			output.appendLine(`** CtrlOEM3 service crashed, ExitCode=${code}. `);
			vscode.window.showErrorMessage('CtrlOEM3 crashed, check output console for details. ', 'Show Output', 'Restart Extension').then((sel) => {
				switch (sel) {
					case 'Show Output':
						output.show();
						break;
					case 'Restart Extension':
						vscode.commands.executeCommand('ctrl-oem3.start');
						break;
				}
			});
		}
	});

	setTimeout(() => {
		let client = net.connect(ID_PIPE_SERVER);
		client.on('connect', () => {
			client.write(Uint8Array.from([ID_PROTO_GET_STATUS]));
		});
		client.on('data', (data) => {
			let router: { [key: number]: () => void } = {
				[ID_PROTO_GRIPE_REGEX]: () => {
					vscode.window.showErrorMessage('Failed to compile `ctrl-oem3.matches-window-title`, check output console for details. ', 'Show Output', 'Edit Settings', 'Restart Extension').then((sel) => {
						switch (sel) {
							case 'Show Output':
								output.show();
								break;
							case 'Edit Settings':
								vscode.commands.executeCommand('workbench.action.openSettings', '@ext:ctrl-oem3.matches-window-title');
								break;
							case 'Restart Extension':
								vscode.commands.executeCommand('ctrl-oem3.stop');
								setTimeout(() => {
									vscode.commands.executeCommand('ctrl-oem3.start');
								}, 500);
								break;
						}
					});
				},
				[ID_PROTO_SAY_OK]: () => {
					output.appendLine('** Connected to CtrlOEM3 service. ');
				},
			};
			for (let cmd of data) {
				if (router[cmd] === undefined) {
					output.appendLine(`** Received unknown ACK=${cmd} from CtrlOEM3 service. `);
				} else {
					router[cmd]();
				}
			}
		});
		client.on('error', (err) => {
			output.appendLine(`** Failed to connect to CtrlOEM3 service: ${err}`);
			vscode.window.showErrorMessage('Failed to connect to CtrlOEM3 service, check output console for details. ', 'Show Output', 'Restart Extension').then((sel) => {
				switch (sel) {
					case 'Restart Extension':
						vscode.commands.executeCommand('ctrl-oem3.start');
						break;
					case 'Show Output':
						output.show();
						break;
				}
			});
		});
		client.on('close', () => {
			output.appendLine('** CtrlOEM3 service disconnected. ');
		});
	}, 500);
}

function command_stop(native_path: string, output: vscode.OutputChannel) {
	let client = net.connect(ID_PIPE_SERVER);
	client.on('connect', () => {
		client.write(Uint8Array.from([ID_PROTO_NOTIFY_STOP]));
		output.appendLine('** CtrlOEM3 service is notified to stop. ');
	});
	client.on('error', (err) => {
		output.appendLine(`** Failed to stop CtrlOEM3 service: ${err}`);
	});
}

