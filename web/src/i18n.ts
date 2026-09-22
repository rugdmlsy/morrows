export type Locale = "zh-CN" | "en";

const translations = {
  "zh-CN": {
    workOs: "工作系统",
    workQueue: "任务队列",
    agentFleet: "Agent 集群",
    localDaemon: "本地守护进程",
    localFirstControlPlane: "本地优先控制平面",
    newTask: "新建任务…",
    priority: "优先级",
    create: "创建",
    dispatchNext: "分配下一个",
    dispatchEnabled: "可调度",
    noTasks: "还没有任务。请在上方创建第一个工作项。",
    noDescription: "暂无描述。",
    context: "上下文",
    revision: "修订版本",
    noExplicitGoal: "未设置明确目标",
    noSummary: "暂无摘要。",
    noContext: "还没有上下文修订。",
    dispatcher: "调度器",
    requiredCapabilities: "所需能力",
    leaseSeconds: "租约秒数",
    enabled: "启用",
    savePolicy: "保存策略",
    preview: "预览",
    dispatch: "分配",
    executor: "执行者",
    heartbeat: "心跳",
    capacity: "容量",
    noDispatchPolicy: "未设置执行者调度策略。",
    selected: "已选择",
    noEligibleAgent: "没有符合条件的 Agent",
    task: "任务",
    effectiveSlots: "有效槽位",
    active: "活跃",
    eligible: "符合条件",
    decisionHistory: "调度决策记录",
    noAgent: "无 Agent",
    assignments: "任务分配",
    leaseUntil: "租约至",
    unassigned: "未分配。",
    runs: "运行记录",
    noExternalSession: "无外部会话",
    noRuns: "还没有运行记录。",
    launcher: "执行器启动",
    launchWith: "使用以下配置启动",
    launch: "启动",
    noLaunchProfile: "这个 Agent 没有可用的启动配置。",
    launchAttempts: "启动记录",
    noLaunchAttempts: "还没有启动记录。",
    externalSession: "外部会话",
    processId: "进程 PID",
    exitCode: "退出码",
    stdoutLog: "标准输出日志",
    stderrLog: "标准错误日志",
    launchError: "启动错误",
    handoffs: "交接",
    completed: "已完成",
    remaining: "剩余",
    blockers: "阻塞项",
    noneRecorded: "无记录",
    none: "无",
    acceptedByRun: "由运行接受",
    noHandoffs: "没有交接记录。",
    artifacts: "产物",
    noArtifacts: "没有产物。",
    decisions: "决策",
    noDecisions: "没有决策记录。",
    discussion: "讨论",
    replyTo: "回复",
    responseRequired: "需要回复",
    noDiscussions: "没有讨论。",
    prerequisites: "前置任务",
    noPrerequisites: "没有前置任务。",
    eventTimeline: "事件时间线",
    noEvents: "没有事件。",
    selectTask: "请选择一个任务。",
    profile: "配置档案",
    account: "账号",
    machine: "机器",
    unknownProvider: "未知提供商",
    notLinked: "未关联",
    general: "通用",
    availableSlots: "可用槽位",
    max: "最大",
    maxNotReported: "未报告最大并发",
    activeAssignments: "活跃分配",
    activeRuns: "活跃运行",
    quota: "额度",
    notReported: "未报告",
    observed: "观测于",
    noCapacity: "未报告容量。",
    noAgents: "还没有注册 Agent 实例。",
    language: "语言",
    switchLanguage: "English",
  },
  en: {
    workOs: "Work OS",
    workQueue: "Work Queue",
    agentFleet: "Agent Fleet",
    localDaemon: "Local daemon",
    localFirstControlPlane: "LOCAL-FIRST CONTROL PLANE",
    newTask: "New task…",
    priority: "Priority",
    create: "Create",
    dispatchNext: "Dispatch next",
    dispatchEnabled: "dispatch",
    noTasks: "No tasks yet. Create the first work item above.",
    noDescription: "No description.",
    context: "Context",
    revision: "Revision",
    noExplicitGoal: "No explicit goal",
    noSummary: "No summary yet.",
    noContext: "No context revision yet.",
    dispatcher: "Dispatcher",
    requiredCapabilities: "Required capabilities",
    leaseSeconds: "Lease seconds",
    enabled: "Enabled",
    savePolicy: "Save policy",
    preview: "Preview",
    dispatch: "Dispatch",
    executor: "executor",
    heartbeat: "heartbeat",
    capacity: "capacity",
    noDispatchPolicy: "No executor dispatch policy.",
    selected: "Selected",
    noEligibleAgent: "No eligible agent",
    task: "Task",
    effectiveSlots: "effective slots",
    active: "active",
    eligible: "eligible",
    decisionHistory: "Decision history",
    noAgent: "no agent",
    assignments: "Assignments",
    leaseUntil: "lease",
    unassigned: "Unassigned.",
    runs: "Runs",
    noExternalSession: "no external session",
    noRuns: "No runs yet.",
    launcher: "Executor Launcher",
    launchWith: "Launch with",
    launch: "Launch",
    noLaunchProfile: "No enabled launch profile for this agent.",
    launchAttempts: "Launch attempts",
    noLaunchAttempts: "No launch attempts yet.",
    externalSession: "External session",
    processId: "Process PID",
    exitCode: "Exit code",
    stdoutLog: "stdout log",
    stderrLog: "stderr log",
    launchError: "Launch error",
    handoffs: "Handoffs",
    completed: "Completed",
    remaining: "Remaining",
    blockers: "Blockers",
    noneRecorded: "None recorded",
    none: "None",
    acceptedByRun: "accepted by run",
    noHandoffs: "No handoffs.",
    artifacts: "Artifacts",
    noArtifacts: "No artifacts.",
    decisions: "Decisions",
    noDecisions: "No decisions.",
    discussion: "Discussion",
    replyTo: "reply to",
    responseRequired: "response required",
    noDiscussions: "No discussions.",
    prerequisites: "Prerequisites",
    noPrerequisites: "No prerequisites.",
    eventTimeline: "Event Timeline",
    noEvents: "No events.",
    selectTask: "Select a task.",
    profile: "Profile",
    account: "Account",
    machine: "Machine",
    unknownProvider: "unknown provider",
    notLinked: "Not linked",
    general: "general",
    availableSlots: "available slots",
    max: "max",
    maxNotReported: "max not reported",
    activeAssignments: "active assignments",
    activeRuns: "active runs",
    quota: "Quota",
    notReported: "not reported",
    observed: "observed",
    noCapacity: "No capacity reported.",
    noAgents: "No agent instances registered yet.",
    language: "Language",
    switchLanguage: "中文",
  },
} as const;

export type TranslationKey = keyof typeof translations["zh-CN"];

export function translate(locale: Locale, key: TranslationKey): string {
  return translations[locale][key];
}

export function initialLocale(): Locale {
  const stored = window.localStorage.getItem("agent-company.locale");
  return stored === "en" ? "en" : "zh-CN";
}

const stateZh: Record<string, string> = {
  backlog: "待办",
  inbox: "收件箱",
  triaged: "已分类",
  ready: "就绪",
  claimed: "已领取",
  in_progress: "进行中",
  review: "审阅中",
  blocked: "阻塞",
  needs_input: "等待输入",
  needs_review: "等待审阅",
  approved: "已批准",
  done: "完成",
  cancelled: "已取消",
  active: "活跃",
  released: "已释放",
  expired: "已过期",
  pending: "待处理",
  queued: "已排队",
  starting: "启动中",
  accepted: "已接受",
  rejected: "已拒绝",
  running: "运行中",
  paused: "已暂停",
  completed: "已完成",
  handed_off: "已交接",
  failed: "失败",
  online: "在线",
  offline: "离线",
  available: "可用",
  unavailable: "不可用",
  throttled: "受限",
  usage_limited: "额度受限",
  exhausted: "已耗尽",
  unknown: "未知",
};

export function formatState(locale: Locale, state: string): string {
  if (locale === "zh-CN") return stateZh[state] ?? state.replaceAll("_", " ");
  return state.replaceAll("_", " ");
}

export function formatRole(locale: Locale, role: string): string {
  if (locale === "zh-CN" && role === "executor") return "执行者";
  if (locale === "zh-CN" && role === "reviewer") return "审阅者";
  if (locale === "zh-CN" && role === "owner") return "负责人";
  return role;
}

export function formatOutcome(locale: Locale, outcome: string): string {
  if (locale !== "zh-CN") return outcome.replaceAll("_", " ");
  const labels: Record<string, string> = {
    assigned: "已分配",
    no_candidate: "无候选 Agent",
  };
  return labels[outcome] ?? outcome.replaceAll("_", " ");
}

export function formatDispatchReason(locale: Locale, reason: string): string {
  if (locale !== "zh-CN") return reason.replaceAll("_", " ");
  const fixed: Record<string, string> = {
    unfinished_dependencies: "前置任务未完成",
    active_assignment_exists: "已有活跃分配",
    policy_disabled: "调度策略已禁用",
    profile_mismatch: "配置档案不匹配",
    account_mismatch: "账号不匹配",
    machine_mismatch: "机器不匹配",
    stale_heartbeat: "心跳已过期",
    stale_capacity: "容量信息已过期",
    missing_capacity: "缺少容量信息",
    no_effective_slots: "没有有效槽位",
  };
  if (fixed[reason]) return fixed[reason];
  const [prefix, ...rest] = reason.split(":");
  const value = rest.join(":");
  if (prefix === "task_state") return "任务状态：" + formatState(locale, value);
  if (prefix === "missing_capability") return "缺少能力：" + value;
  if (prefix === "instance_status") return "实例状态：" + formatState(locale, value);
  if (prefix === "capacity_status") return "容量状态：" + formatState(locale, value);
  if (prefix === "quota_state") return "额度状态：" + formatState(locale, value);
  return reason.replaceAll("_", " ");
}

export function formatAge(locale: Locale, iso: string): string {
  const seconds = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000));
  if (locale === "zh-CN") {
    if (seconds < 60) return seconds + " 秒前";
    if (seconds < 3600) return Math.floor(seconds / 60) + " 分钟前";
    if (seconds < 86400) return Math.floor(seconds / 3600) + " 小时前";
    return Math.floor(seconds / 86400) + " 天前";
  }
  if (seconds < 60) return seconds + "s ago";
  if (seconds < 3600) return Math.floor(seconds / 60) + "m ago";
  if (seconds < 86400) return Math.floor(seconds / 3600) + "h ago";
  return Math.floor(seconds / 86400) + "d ago";
}
