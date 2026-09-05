#!/usr/bin/env python3
"""Export the elaborated slang AST into VTR's independent RTL KDB v2."""
from collections import Counter
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import sys
import pyslang as slang


def tag(n):
    return str(n.kind).split('.')[-1]


class Exporter:
    def __init__(self, compilation):
        self.sm = compilation.sourceManager
        self.symbols = {}
        self.instances = []
        self.processes = []
        self.connections = []

    def owner(self, n):
        scope = n.parentScope
        instance = scope.containingInstance if scope else None
        return instance.hierarchicalPath if instance else None

    def loc(self, n):
        l = n.location if hasattr(n, 'location') else n.sourceRange.start
        return dict(file=str(self.sm.getFileName(l)), line=self.sm.getLineNumber(l),
                    column=self.sm.getColumnNumber(l))

    def typ(self, t):
        return dict(text=str(t), width=t.bitWidth, signed=t.isSigned,
                    four_state=t.isFourState, integral=t.isIntegral)

    def expr(self, e):
        d = dict(kind=tag(e), type=self.typ(e.type), source=self.loc(e))
        k = d['kind']
        if k in ('NamedValue', 'HierarchicalValue'):
            if isinstance(e.symbol, (slang.ast.ParameterSymbol, slang.ast.EnumValueSymbol)):
                d.update(kind='Constant', value=str(e.symbol.value))
            else:
                d['symbol'] = e.symbol.hierarchicalPath
        elif k in ('IntegerLiteral', 'UnbasedUnsizedIntegerLiteral'):
            d.update(kind='Constant', value=str(e.value))
        elif k in ('BinaryOp',):
            d.update(op=str(e.op).split('.')[-1], left=self.expr(e.left), right=self.expr(e.right))
        elif k in ('UnaryOp', 'Conversion'):
            d.update(operand=self.expr(e.operand))
            if k == 'UnaryOp':
                d['op'] = str(e.op).split('.')[-1]
        elif k == 'ConditionalOp' and len(e.conditions) == 1 and e.conditions[0].pattern is None:
            d.update(cond=self.expr(e.conditions[0].expr), yes=self.expr(e.left), no=self.expr(e.right))
        elif k == 'Concatenation':
            d['operands'] = [self.expr(x) for x in e.operands]
        elif k == 'Replication':
            d.update(count=int(str(e.count.constant)), operand=self.expr(e.concat))
        elif k == 'RangeSelect':
            r = e.value.type.getBitVectorRange()
            d.update(value=self.expr(e.value), left=self.expr(e.left), right=self.expr(e.right),
                     selection=str(e.selectionKind).split('.')[-1], range_left=r.left, range_right=r.right)
        elif k == 'ElementSelect':
            d.update(value=self.expr(e.value), selector=self.expr(e.selector))
            r = e.value.type.getBitVectorRange()
            d.update(range_left=r.left, range_right=r.right)
        else:
            d.update(kind='Unsupported', reason=f'expression {k}', reads=self.reads(e))
        return d

    def assignment(self, e):
        if tag(e) != 'Assignment' or e.isCompound or e.timingControl is not None:
            return dict(kind='Unsupported', reason='compound or timed assignment', source=self.loc(e))
        lhs = self.expr(e.left)
        if lhs['kind'] not in ('NamedValue', 'HierarchicalValue'):
            return dict(kind='Unsupported', reason='partial/aggregate assignment target', source=self.loc(e))
        return dict(kind='Assign', target=lhs['symbol'], value=self.expr(e.right),
                    nba=e.isNonBlocking, source=self.loc(e))

    def stmt(self, s):
        if s is None:
            return dict(kind='Empty')
        k = tag(s)
        if k == 'Block' and str(s.blockKind).endswith('Sequential'):
            return self.stmt(s.body)
        if k == 'List':
            return dict(kind='Sequence', statements=[self.stmt(x) for x in s.list])
        if k == 'Empty':
            return dict(kind='Empty')
        if k == 'ExpressionStatement':
            return self.assignment(s.expr)
        if k == 'Conditional' and len(s.conditions) == 1 and s.conditions[0].pattern is None:
            return dict(kind='If', cond=self.expr(s.conditions[0].expr), yes=self.stmt(s.ifTrue),
                        no=self.stmt(s.ifFalse), source=self.loc(s))
        return dict(kind='Unsupported', reason=f'statement {k}', source=self.loc(s))

    def targets(self, n):
        out = set()
        def visit(x):
            if isinstance(x, slang.ast.AssignmentExpression):
                def lhs(y):
                    if isinstance(y, (slang.ast.NamedValueExpression, slang.ast.HierarchicalValueExpression)):
                        out.add(y.symbol.hierarchicalPath)
                x.left.visit(lhs)
        n.visit(visit)
        return sorted(out)

    def reads(self, n):
        # Count references then subtract assignment LHS references; this preserves
        # self-feedback and reads hidden inside unsupported statements.
        refs = Counter()
        def ref(x, weight=1):
            if isinstance(x, (slang.ast.NamedValueExpression, slang.ast.HierarchicalValueExpression)):
                if not isinstance(x.symbol, (slang.ast.ParameterSymbol, slang.ast.EnumValueSymbol)):
                    refs[x.symbol.hierarchicalPath] += weight
        def visit(x):
            ref(x)
            if isinstance(x, slang.ast.AssignmentExpression):
                # Keep selectors on partial writes as reads, remove only the base.
                lhs = x.left
                while isinstance(lhs, (slang.ast.ElementSelectExpression, slang.ast.RangeSelectExpression)):
                    lhs = lhs.value
                if not x.isCompound:
                    ref(lhs, -1)
        n.visit(visit)
        return sorted(k for k, count in refs.items() if count > 0)

    def process(self, n):
        events = []
        if tag(n) == 'ContinuousAssign':
            body = self.assignment(n.assignment)
            mode = 'comb'
            if n.delay is not None:
                body = dict(kind='Unsupported', reason='continuous assignment delay', source=self.loc(n))
        else:
            s = n.body
            mode = 'comb' if str(n.procedureKind).endswith('AlwaysComb') else 'unsupported'
            if tag(s) == 'Timed':
                timing = s.timing
                if tag(timing) == 'ImplicitEvent':
                    mode = 'comb'
                else:
                    ts = timing.events if tag(timing) == 'EventList' else [timing]
                    mode = 'seq'
                    for t in ts:
                        if tag(t) != 'SignalEvent' or str(t.edge).split('.')[-1] not in ('PosEdge', 'NegEdge') or t.iffCondition is not None:
                            mode = 'unsupported'
                            break
                        events.append(dict(edge=str(t.edge).split('.')[-1], expr=self.expr(t.expr), source=self.loc(t)))
                s = s.stmt
            body = self.stmt(s)
        self.processes.append(dict(id=len(self.processes), owner=self.owner(n), origin="rtl", reads=self.reads(n), mode=mode, targets=self.targets(n),
                                   events=events, body=body, source=self.loc(n)))

    def visit(self, n):
        k = tag(n)
        if k in ('Variable', 'Net', 'Parameter', 'EnumValue'):
            d = dict(path=n.hierarchicalPath, owner=self.owner(n), kind=k, type=self.typ(n.type), source=self.loc(n))
            if k in ('Parameter', 'EnumValue'):
                d['value'] = str(n.value)
            self.symbols[n.hierarchicalPath] = d
            if k == 'Net' and n.initializer is not None:
                body = dict(kind='Assign', target=n.hierarchicalPath, value=self.expr(n.initializer), nba=False, source=self.loc(n))
                if n.delay is not None:
                    body = dict(kind='Unsupported', reason='net declaration delay', source=self.loc(n))
                self.processes.append(dict(id=len(self.processes), owner=self.owner(n), origin='rtl', reads=self.reads(n.initializer),
                                           mode='comb', targets=[n.hierarchicalPath], events=[], body=body, source=self.loc(n)))
            if k == 'Variable' and n.initializer is not None:
                self.processes.append(dict(id=len(self.processes), owner=self.owner(n), origin='rtl', reads=self.reads(n.initializer), mode='unsupported', targets=[n.hierarchicalPath], events=[], body=dict(kind='Unsupported', reason='variable initializer'), source=self.loc(n)))
        elif k == 'Instance':
            ports = []
            for p in n.body.portList:
                if hasattr(p, 'internalSymbol') and p.internalSymbol is not None:
                    ports.append(dict(name=p.name, symbol=p.internalSymbol.hierarchicalPath,
                                      direction=str(p.direction).split('.')[-1]))
                else:
                    raise RuntimeError(f'unsupported module port at {n.hierarchicalPath}.{p.name}')
            self.instances.append(dict(path=n.hierarchicalPath, parent=self.owner(n),
                                       ports=ports, definition=n.definition.name, source=self.loc(n)))
            for c in n.portConnections:
                p = c.port
                if p.internalSymbol is None or c.expression is None:
                    continue
                inside = p.internalSymbol.hierarchicalPath
                e = self.expr(c.expression)
                direction = str(p.direction).split('.')[-1]
                target, value = inside, e
                if direction in ('Out', 'InOut', 'Ref'):
                    # slang represents output connection as an assignment to the parent.
                    a = c.expression
                    if tag(a) == 'Assignment':
                        e = self.expr(a.left)
                        target = e.get('symbol', '')
                    else:
                        target = e.get('symbol', '')
                    value = dict(kind='NamedValue', symbol=inside, type=self.typ(p.type), source=self.loc(p))
                self.connections.append(dict(instance=n.hierarchicalPath, port=inside, direction=direction, expression=e, source=self.loc(p)))
                body = dict(kind='Assign', target=target, value=value, nba=False, source=self.loc(p))
                if direction not in ('In', 'Out') or not target:
                    body = dict(kind='Unsupported', reason='inout/ref or complex output connection', source=self.loc(p))
                targets = [target] if target else (self.targets(c.expression) or [inside])
                self.processes.append(dict(id=len(self.processes), owner=self.owner(n), origin='connection', reads=[], mode='comb', targets=targets, events=[], body=body, source=self.loc(p)))
        elif k in ('ContinuousAssign', 'ProceduralBlock'):
            self.process(n)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('-o', '--output', required=True)
    p.add_argument('--top', required=True)
    p.add_argument('-I', dest='includes', action='append', default=[])
    p.add_argument('-D', dest='defines', action='append', default=[])
    p.add_argument('sources', nargs='+')
    args = p.parse_args()
    if slang.__version__ != '11.0.0':
        p.error('requires pyslang==11.0.0 (tools/kdb/requirements.txt)')
    driver = slang.driver.Driver()
    driver.addStandardArgs()
    cmd = ['slang', '--top', args.top] + ['-I'+x for x in args.includes] + ['-D'+x for x in args.defines] + args.sources
    if not driver.parseCommandLine(shlex.join(cmd)) or not driver.processOptions() or not driver.parseAllSources():
        raise RuntimeError('slang source loading failed')
    c = driver.createCompilation()
    root = c.getRoot()
    diagnostics = c.getAllDiagnostics()
    if diagnostics:
        print(slang.DiagnosticEngine.reportAll(c.sourceManager, diagnostics), file=sys.stderr)
    if c.hasIssuedErrors or c.hasFatalErrors:
        raise RuntimeError('slang elaboration failed')
    if [i.name for i in root.topInstances] != [args.top]:
        raise RuntimeError('requested top did not elaborate uniquely')
    ex = Exporter(c)
    root.visit(ex.visit)
    sources = []
    for b in c.sourceManager.getAllBuffers():
        name = str(c.sourceManager.getFullPath(b))
        if name and Path(name).is_file():
            sources.append(dict(path=str(Path(name).relative_to(Path.cwd())) if Path(name).is_relative_to(Path.cwd()) else name,
                                sha256=hashlib.sha256(Path(name).read_bytes()).hexdigest()))
    doc = dict(format='vtr-rtl-kdb', version=2, producer='pyslang '+slang.__version__, top=args.top,
               sources=sorted(sources, key=lambda x:x['path']), options=cmd[1:],
               instances=ex.instances, symbols=ex.symbols, connections=ex.connections, processes=ex.processes)
    doc['design_id'] = hashlib.sha256(json.dumps(doc, sort_keys=True).encode()).hexdigest()
    Path(args.output).write_text(json.dumps(doc, indent=2, sort_keys=True)+'\n')


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, OSError) as e:
        sys.exit(f'error: {e}')
