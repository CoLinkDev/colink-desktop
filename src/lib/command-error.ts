export interface CommandError {
  kind: 'network' | 'http' | 'protocol' | 'application'
  code: number | null
  status: number | null
  message: string
}

export function readCommandError(error: unknown): CommandError | null {
  if (!error || typeof error !== 'object') {
    return null
  }

  const candidate = error as Partial<CommandError>
  if (
    typeof candidate.kind !== 'string' ||
    typeof candidate.message !== 'string' ||
    candidate.code !== null && typeof candidate.code !== 'number' ||
    candidate.status !== null && typeof candidate.status !== 'number'
  ) {
    return null
  }

  return candidate as CommandError
}

export function hasProtocolCode(error: unknown, code: number) {
  const commandError = readCommandError(error)
  return commandError?.kind === 'protocol' && commandError.code === code
}

export function hasHttpStatus(error: unknown, status: number) {
  const commandError = readCommandError(error)
  return commandError?.kind === 'http' && commandError.status === status
}

export function isNetworkError(error: unknown) {
  return readCommandError(error)?.kind === 'network'
}
