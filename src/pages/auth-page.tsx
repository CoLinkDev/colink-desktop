import type { FormEvent, ReactNode } from 'react'
import { useState } from 'react'
import { Navigate, useNavigate } from 'react-router-dom'
import { z } from 'zod'

import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'

const loginSchema = z.object({
  identifier: z.string().min(1, '请输入邮箱或用户名'),
  password: z.string().min(1, '请输入密码'),
})

const registerSchema = z
  .object({
    email: z.string().email('邮箱格式不对'),
    username: z
      .string()
      .min(3, '用户名至少 3 位')
      .max(32, '用户名不能超过 32 位'),
    password: z.string().min(8, '密码至少 8 位'),
    confirmPassword: z.string().min(8, '请再次输入密码'),
  })
  .refine((value) => value.password === value.confirmPassword, {
    message: '两次密码不一致',
    path: ['confirmPassword'],
  })

export function AuthPage() {
  const navigate = useNavigate()
  const { session, status, login, register, bootstrapError } = useAppState()
  const [mode, setMode] = useState<'login' | 'register'>('login')
  const [error, setError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [loginForm, setLoginForm] = useState({
    identifier: '',
    password: '',
  })
  const [registerForm, setRegisterForm] = useState({
    email: '',
    username: '',
    password: '',
    confirmPassword: '',
  })

  if (status === 'booting') {
    return null
  }

  if (session) {
    return <Navigate replace to="/devices" />
  }

  async function handleLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)

    const parsed = loginSchema.safeParse(loginForm)

    if (!parsed.success) {
      setError(parsed.error.issues[0]?.message ?? '请输入完整信息')
      return
    }

    setSubmitting(true)

    try {
      await login(parsed.data)
      navigate('/devices')
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setSubmitting(false)
    }
  }

  async function handleRegister(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)

    const parsed = registerSchema.safeParse(registerForm)

    if (!parsed.success) {
      setError(parsed.error.issues[0]?.message ?? '请输入完整信息')
      return
    }

    setSubmitting(true)

    try {
      await register({
        email: parsed.data.email,
        username: parsed.data.username,
        password: parsed.data.password,
      })
      navigate('/devices')
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="grid min-h-screen bg-[hsl(var(--background))] px-6 py-8 lg:grid-cols-[minmax(0,480px)_minmax(0,1fr)]">
      <section className="flex items-center justify-center">
        <div className="w-full max-w-md rounded-lg border border-[hsl(var(--border))] bg-[hsl(var(--panel))] p-8">
          <div className="text-xs uppercase tracking-[0.12em] text-[hsl(var(--muted))]">
            CoLink Desktop
          </div>
          <h1 className="mt-3 text-2xl font-semibold">账户连接</h1>
          <p className="mt-2 text-sm text-[hsl(var(--muted))]">
            先接入账户，再同步设备状态。
          </p>

          <div className="mt-6 grid grid-cols-2 gap-2 rounded-lg border border-[hsl(var(--border))] p-1">
            <button
              className={
                mode === 'login'
                  ? 'rounded-md bg-[hsl(var(--panel-2))] px-3 py-2 text-sm text-[hsl(var(--text))]'
                  : 'rounded-md px-3 py-2 text-sm text-[hsl(var(--muted))]'
              }
              onClick={() => setMode('login')}
              type="button"
            >
              登录
            </button>
            <button
              className={
                mode === 'register'
                  ? 'rounded-md bg-[hsl(var(--panel-2))] px-3 py-2 text-sm text-[hsl(var(--text))]'
                  : 'rounded-md px-3 py-2 text-sm text-[hsl(var(--muted))]'
              }
              onClick={() => setMode('register')}
              type="button"
            >
              注册
            </button>
          </div>

          {(bootstrapError || error) && (
            <div className="mt-6 rounded-lg border border-[hsl(var(--danger)/0.5)] bg-[hsl(var(--danger)/0.12)] px-4 py-3 text-sm text-[hsl(var(--text))]">
              {error ?? bootstrapError}
            </div>
          )}

          {mode === 'login' ? (
            <form className="mt-6 space-y-4" onSubmit={handleLogin}>
              <Field label="邮箱或用户名">
                <Input
                  autoComplete="username"
                  onChange={(event) =>
                    setLoginForm((current) => ({
                      ...current,
                      identifier: event.target.value,
                    }))
                  }
                  placeholder="user@example.com"
                  value={loginForm.identifier}
                />
              </Field>

              <Field label="密码">
                <Input
                  autoComplete="current-password"
                  onChange={(event) =>
                    setLoginForm((current) => ({
                      ...current,
                      password: event.target.value,
                    }))
                  }
                  placeholder="请输入密码"
                  type="password"
                  value={loginForm.password}
                />
              </Field>

              <Button className="w-full" disabled={submitting} type="submit">
                {submitting ? '正在登录' : '登录'}
              </Button>
            </form>
          ) : (
            <form className="mt-6 space-y-4" onSubmit={handleRegister}>
              <Field label="邮箱">
                <Input
                  autoComplete="email"
                  onChange={(event) =>
                    setRegisterForm((current) => ({
                      ...current,
                      email: event.target.value,
                    }))
                  }
                  placeholder="user@example.com"
                  value={registerForm.email}
                />
              </Field>

              <Field label="用户名">
                <Input
                  autoComplete="username"
                  onChange={(event) =>
                    setRegisterForm((current) => ({
                      ...current,
                      username: event.target.value,
                    }))
                  }
                  placeholder="brook.user"
                  value={registerForm.username}
                />
              </Field>

              <Field label="密码">
                <Input
                  autoComplete="new-password"
                  onChange={(event) =>
                    setRegisterForm((current) => ({
                      ...current,
                      password: event.target.value,
                    }))
                  }
                  placeholder="至少 8 位"
                  type="password"
                  value={registerForm.password}
                />
              </Field>

              <Field label="确认密码">
                <Input
                  autoComplete="new-password"
                  onChange={(event) =>
                    setRegisterForm((current) => ({
                      ...current,
                      confirmPassword: event.target.value,
                    }))
                  }
                  placeholder="再次输入密码"
                  type="password"
                  value={registerForm.confirmPassword}
                />
              </Field>

              <Button className="w-full" disabled={submitting} type="submit">
                {submitting ? '正在创建账户' : '注册并登录'}
              </Button>
            </form>
          )}
        </div>
      </section>

      <section className="hidden border-l border-[hsl(var(--border))] px-12 lg:flex lg:flex-col lg:justify-center">
        <div className="max-w-lg">
          <div className="text-sm uppercase tracking-[0.14em] text-[hsl(var(--muted))]">
            桌面端第一版
          </div>
          <div className="mt-4 text-4xl font-semibold leading-tight">
            先把账户、设备注册和本地状态链路接通。
          </div>
          <div className="mt-5 text-base text-[hsl(var(--muted))]">
            这一版覆盖登录、注册、设备拉取、设置持久化。LAN、云端 WS、文件传输还没接进来。
          </div>
        </div>
      </section>
    </div>
  )
}

interface FieldProps {
  label: string
  children: ReactNode
}

function Field({ label, children }: FieldProps) {
  return (
    <label className="block space-y-2">
      <span className="text-sm text-[hsl(var(--muted))]">{label}</span>
      {children}
    </label>
  )
}
