-- Convert the server-rendered Docusaurus manual to plain Markdown for VitePress.
-- The source remains on the existing Nginx site; this removes site chrome only.
function Div(element)
  if element.classes:includes("tocCollapsible_ETCw")
      or element.classes:includes("theme-doc-toc-desktop") then
    return {}
  end
  return element.content
end

function Span(element)
  return element.content
end

function Link(element)
  if element.classes:includes("hash-link") then
    return {}
  end
  return element
end

function Pandoc(document)
  local content = {}
  local started = false
  for _, block in ipairs(document.blocks) do
    if block.t == "Header" and block.level == 1 then
      started = true
    end
    if started then
      table.insert(content, block)
    end
  end
  document.blocks = content
  return document
end
