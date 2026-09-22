"""GitHub MCP stdio server using the user's existing gh authentication."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

STRING = {'type': 'string', 'minLength': 1}
OPERATIONS = {
    'get_repository': ('GET', '', {}, []),
    'list_issues': ('GET', '/issues', {'page': {'type': 'integer', 'minimum': 1}}, []),
    'list_pull_requests': ('GET', '/pulls', {'page': {'type': 'integer', 'minimum': 1}}, []),
    'list_workflow_runs': ('GET', '/actions/runs', {'page': {'type': 'integer', 'minimum': 1}}, []),
    'create_issue': ('POST', '/issues', {'title': STRING, 'body': STRING}, ['title', 'body']),
    'create_pull_request': ('POST', '/pulls', {'title': STRING, 'body': STRING, 'head': STRING, 'base': STRING}, ['title', 'body', 'head', 'base']),
    'dispatch_workflow': ('POST', '/actions/workflows/{workflow}/dispatches', {'workflow': STRING, 'ref': STRING, 'inputs': {'type': 'object', 'additionalProperties': {'type': 'string'}}}, ['workflow', 'ref']),
}


def tool_list():
    return [{'name': name, 'description': name.replace('_', ' ') + ' on GitHub. List results are paginated, 30 per page.',
             'inputSchema': {'type': 'object', 'properties': {'owner': STRING, 'repo': STRING, **fields},
                             'required': ['owner', 'repo', *required], 'additionalProperties': False},
             'annotations': {'readOnlyHint': method == 'GET'}}
            for name, (method, _, fields, required) in OPERATIONS.items()]


def invoke(name, arguments):
    if name not in OPERATIONS or not isinstance(arguments, dict):
        raise ValueError('Unknown tool or invalid arguments')
    method, suffix, fields, required = OPERATIONS[name]
    allowed = {'owner', 'repo', *fields}
    if set(arguments) - allowed or any(key not in arguments for key in ['owner', 'repo', *required]):
        raise ValueError('Missing or unexpected arguments')
    for key, value in arguments.items():
        if key == 'page':
            if type(value) is not int or not 1 <= value <= 10000:
                raise ValueError('page must be an integer from 1 to 10000')
        elif key == 'inputs':
            if not isinstance(value, dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in value.items()):
                raise ValueError('inputs must contain string values')
        elif not isinstance(value, str) or not value.strip() or len(value) > 65536:
            raise ValueError('Expected nonempty string argument')
    for key in ('owner', 'repo', 'workflow'):
        if key in arguments and (not re.fullmatch(r'[A-Za-z0-9_.-]+', arguments[key]) or arguments[key] in ('.', '..')):
            raise ValueError('Invalid repository or workflow identifier')
    endpoint = '/repos/' + arguments['owner'] + '/' + arguments['repo'] + suffix.format(**arguments)
    payload = {key: value for key, value in arguments.items() if key not in ('owner', 'repo', 'workflow', 'page')}
    if method == 'GET' and suffix:
        endpoint += '?per_page=30&page=' + str(arguments.get('page', 1))
    command = ['gh', 'api', '--hostname', 'github.com', '--method', method, endpoint]
    if method == 'POST':
        command += ['--input', '-']
    environment = os.environ.copy()
    environment.setdefault("HOME", str(Path.home()))
    result = subprocess.run(command, input=json.dumps(payload) if method == 'POST' else None,
                            capture_output=True, text=True, timeout=60, env=environment)
    if result.returncode:
        raise ValueError('GitHub request failed. Check gh auth status, repository permissions and rate limits.')
    if len(result.stdout) > 2 * 1024 * 1024:
        raise ValueError('GitHub response exceeds the 2 MiB limit')
    return json.loads(result.stdout) if result.stdout.strip() else {'ok': True}


def handle(request):
    method = request.get('method')
    if method == 'initialize':
        return {'protocolVersion': '2025-06-18', 'capabilities': {'tools': {}},
                'serverInfo': {'name': 'openforge-github', 'version': '0.2.0'}}
    if method == 'ping':
        return {}
    if method == 'tools/list':
        return {'tools': tool_list()}
    if method == 'tools/call':
        params = request.get('params', {})
        try:
            result = invoke(params.get('name'), params.get('arguments', {}))
            return {'content': [{'type': 'text', 'text': json.dumps(result)}], 'isError': False}
        except (ValueError, OSError, subprocess.SubprocessError):
            return {'content': [{'type': 'text', 'text': 'GitHub call failed: check arguments, gh auth status, repository permissions and rate limits.'}], 'isError': True}
    raise ValueError('Unsupported method')


def main():
    for line in sys.stdin:
        request = None
        try:
            request = json.loads(line)
            if not isinstance(request, dict):
                raise ValueError('Invalid request')
            if 'id' not in request:
                continue
            response = {'jsonrpc': '2.0', 'id': request['id'], 'result': handle(request)}
        except (ValueError, TypeError, AttributeError):
            response = {'jsonrpc': '2.0', 'id': request.get('id') if isinstance(request, dict) else None,
                        'error': {'code': -32600, 'message': 'Invalid or unsupported request'}}
        print(json.dumps(response), flush=True)


if __name__ == '__main__':
    main()
